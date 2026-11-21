//! 元に戻す・やり直し。
//!
//! 変更を打ち消す SQL は、マイグレーション 004 のトリガーが undo_journal に書き留める。
//! ここでは「1 回の操作」をステップとしてまとめ、そのステップ単位で打ち消し SQL を実行する。
//! 画面・MCP のどの入口から変更しても、`record` で包めば元に戻せる。

use rusqlite::{Connection, OptionalExtension, params};

use super::{ServiceResult, Tx};

/// 元に戻せる操作の数の上限
const MAX_STEPS: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stack {
    Undo,
    Redo,
}

impl Stack {
    fn as_str(self) -> &'static str {
        match self {
            Stack::Undo => "undo",
            Stack::Redo => "redo",
        }
    }
}

/// `f` の中で行った変更を 1 ステップとして記録する。新しい操作をしたらやり直しの履歴は消える
pub fn record<T>(
    conn: &Connection,
    label: &str,
    f: impl FnOnce(&Connection) -> ServiceResult<T>,
) -> ServiceResult<T> {
    // 1 回の操作はまるごと成功させるか、まるごと取り消す（途中で失敗したら記録ごと残さない）
    let tx = Tx::begin(conn)?;
    let step: i64 = tx.query_row(
        "SELECT COALESCE(MAX(step), 0) + 1 FROM \
         (SELECT step FROM undo_step UNION ALL SELECT step FROM undo_journal)",
        [],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE undo_state SET mode = 'record', step = ?1 WHERE id = 1",
        [step],
    )?;
    let value = f(&tx)?; // 失敗したら tx を捨てて取り消す
    tx.execute(
        "UPDATE undo_state SET mode = NULL, step = NULL WHERE id = 1",
        [],
    )?;

    let recorded: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM undo_journal WHERE stack = 'undo' AND step = ?1)",
        [step],
        |r| r.get(0),
    )?;
    if recorded {
        tx.execute("DELETE FROM undo_journal WHERE stack = 'redo'", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO undo_step (step, label) VALUES (?1, ?2)",
            params![step, label],
        )?;
        // 古いステップを捨てる
        tx.execute(
            "DELETE FROM undo_journal WHERE stack = 'undo' AND step <= \
             (SELECT step FROM undo_step ORDER BY step DESC LIMIT 1 OFFSET ?1)",
            [MAX_STEPS],
        )?;
        tx.execute(
            "DELETE FROM undo_step WHERE step NOT IN (SELECT step FROM undo_journal)",
            [],
        )?;
    }
    tx.commit()?;
    Ok(value)
}

/// 直前の操作を元に戻す。戻せたときは操作の名前を返す
pub fn undo(conn: &Connection) -> ServiceResult<Option<String>> {
    apply(conn, Stack::Undo)
}

/// 元に戻した操作をやり直す
pub fn redo(conn: &Connection) -> ServiceResult<Option<String>> {
    apply(conn, Stack::Redo)
}

fn apply(conn: &Connection, from: Stack) -> ServiceResult<Option<String>> {
    // 元に戻すのは一番新しいステップ、やり直すのは一番最近に戻したステップ
    let order = if from == Stack::Undo { "DESC" } else { "ASC" };
    let step: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT step FROM undo_journal WHERE stack = ?1 ORDER BY step {order} LIMIT 1"
            ),
            [from.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(step) = step else {
        return Ok(None);
    };

    let tx = Tx::begin(conn)?;
    // 打ち消しの途中では親子の順序が前後するので、外部キーの確認はコミット時に行う
    tx.pragma_update(None, "defer_foreign_keys", "ON")?;
    let statements: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT sql FROM undo_journal WHERE stack = ?1 AND step = ?2 ORDER BY seq DESC",
        )?;
        stmt.query_map(params![from.as_str(), step], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    tx.execute(
        "DELETE FROM undo_journal WHERE stack = ?1 AND step = ?2",
        params![from.as_str(), step],
    )?;
    // 実行中の変更は反対側の履歴に記録される（undo 中 → redo へ、redo 中 → undo へ）
    tx.execute(
        "UPDATE undo_state SET mode = ?1, step = ?2 WHERE id = 1",
        params![from.as_str(), step],
    )?;
    for sql in &statements {
        tx.execute_batch(sql)?;
    }
    tx.execute(
        "UPDATE undo_state SET mode = NULL, step = NULL WHERE id = 1",
        [],
    )?;
    let label: Option<String> = tx
        .query_row("SELECT label FROM undo_step WHERE step = ?1", [step], |r| {
            r.get(0)
        })
        .optional()?;
    tx.commit()?;
    Ok(Some(label.unwrap_or_else(|| "操作".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{MoveTarget, NewTask, Task, TaskPatch};
    use crate::service::task_service as ts;

    fn add(conn: &Connection, title: &str, parent_id: Option<i64>) -> Task {
        record(conn, "追加", |c| {
            ts::create_task(
                c,
                NewTask {
                    title: title.into(),
                    parent_id,
                    ..Default::default()
                },
            )
        })
        .unwrap()
    }

    fn snapshot(conn: &Connection) -> (Vec<Task>, i64) {
        let tasks = ts::list_tasks(conn).unwrap();
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_event", [], |r| r.get(0))
            .unwrap();
        (tasks, events)
    }

    #[test]
    fn undo_and_redo_update() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let before = snapshot(&conn);
        let patch: TaskPatch =
            serde_json::from_str(r#"{"title":"b","planned_end":"2026-11-30","tags":["x"]}"#)
                .unwrap();
        record(&conn, "変更", |c| ts::update_task(c, a.id, patch)).unwrap();
        let after = snapshot(&conn);

        assert_eq!(undo(&conn).unwrap().as_deref(), Some("変更"));
        assert_eq!(snapshot(&conn), before);
        assert!(ts::list_tags(&conn).unwrap().is_empty());
        assert_eq!(redo(&conn).unwrap().as_deref(), Some("変更"));
        assert_eq!(snapshot(&conn), after);
    }

    #[test]
    fn undo_delete_restores_subtree_tags_and_history() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None);
        let c = add(&conn, "子", Some(p.id));
        add(&conn, "孫", Some(c.id));
        let patch: TaskPatch = serde_json::from_str(r#"{"tags":["案件A"]}"#).unwrap();
        record(&conn, "タグ", |cn| ts::update_task(cn, c.id, patch)).unwrap();
        let before = snapshot(&conn);

        record(&conn, "削除", |cn| ts::delete_task(cn, p.id)).unwrap();
        assert!(ts::list_tasks(&conn).unwrap().is_empty());

        undo(&conn).unwrap();
        assert_eq!(snapshot(&conn), before);
        redo(&conn).unwrap();
        assert!(ts::list_tasks(&conn).unwrap().is_empty());
        undo(&conn).unwrap();
        assert_eq!(snapshot(&conn), before);
    }

    #[test]
    fn undo_move_and_create_in_order() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let b = add(&conn, "b", None);
        let s1 = snapshot(&conn);
        record(&conn, "移動", |c| {
            ts::move_task(
                c,
                b.id,
                MoveTarget {
                    parent_id: Some(a.id),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        undo(&conn).unwrap();
        assert_eq!(snapshot(&conn), s1);
        undo(&conn).unwrap(); // b の追加
        undo(&conn).unwrap(); // a の追加
        assert!(ts::list_tasks(&conn).unwrap().is_empty());
        assert_eq!(undo(&conn).unwrap(), None);
        redo(&conn).unwrap();
        redo(&conn).unwrap();
        assert_eq!(snapshot(&conn), s1);
    }

    #[test]
    fn new_action_clears_redo() {
        let conn = db::open_in_memory().unwrap();
        add(&conn, "a", None);
        undo(&conn).unwrap();
        add(&conn, "b", None);
        assert_eq!(redo(&conn).unwrap(), None);
        let titles: Vec<_> = ts::list_tasks(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["b"]);
    }

    #[test]
    fn failed_action_records_nothing() {
        let conn = db::open_in_memory().unwrap();
        add(&conn, "a", None);
        let r = record(&conn, "失敗", |c| ts::create_task(c, NewTask::default()));
        assert!(r.is_err());
        // 直前の「a の追加」が戻る
        assert_eq!(undo(&conn).unwrap().as_deref(), Some("追加"));
    }
}
