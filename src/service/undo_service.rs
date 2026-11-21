//! 元に戻す・やり直し。
//!
//! 変更を打ち消す SQL は、マイグレーション 004 のトリガーが undo_journal に書き留める。
//! ここでは「1 回の操作」をステップとしてまとめ、そのステップ単位で打ち消し SQL を実行する。
//! 画面・MCP のどの入口から変更しても、`record` で包めば元に戻せる。

use rusqlite::{Connection, OptionalExtension, params};

use super::{ServiceResult, Tx};

/// 元に戻せる操作の数の上限
const MAX_STEPS: i64 = 200;

str_enum! {
    /// undo_journal のどちらの山か（マイグレーション 004 のトリガーの 'undo' / 'redo' と同じ文字列）
    enum Stack {
        Undo => "undo",
        Redo => "redo",
    }
}

str_enum! {
    /// undo_state.mode：いま何をしているか（トリガーはこれを見て記録先を決める）
    enum UndoMode {
        /// 通常の操作（打ち消し SQL を undo の山へ）
        Record => "record",
        /// 元に戻している（打ち消し SQL を redo の山へ）
        Undo => "undo",
        /// やり直している（打ち消し SQL を undo の山へ）
        Redo => "redo",
    }
}

impl Stack {
    /// この山から取り出して実行している間のモード
    fn applying_mode(self) -> UndoMode {
        match self {
            Stack::Undo => UndoMode::Undo,
            Stack::Redo => UndoMode::Redo,
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
        "UPDATE undo_state SET mode = ?1, step = ?2 WHERE id = 1",
        params![UndoMode::Record, step],
    )?;
    let value = f(&tx)?; // 失敗したら tx を捨てて取り消す
    tx.execute(
        "UPDATE undo_state SET mode = NULL, step = NULL WHERE id = 1",
        [],
    )?;

    let recorded: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM undo_journal WHERE stack = ?1 AND step = ?2)",
        params![Stack::Undo, step],
        |r| r.get(0),
    )?;
    if recorded {
        tx.execute("DELETE FROM undo_journal WHERE stack = ?1", [Stack::Redo])?;
        tx.execute(
            "INSERT OR REPLACE INTO undo_step (step, label) VALUES (?1, ?2)",
            params![step, label],
        )?;
        // 古いステップを捨てる
        tx.execute(
            "DELETE FROM undo_journal WHERE stack = ?1 AND step <= \
             (SELECT step FROM undo_step ORDER BY step DESC LIMIT 1 OFFSET ?2)",
            params![Stack::Undo, MAX_STEPS],
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
    let order = match from {
        Stack::Undo => "DESC",
        Stack::Redo => "ASC",
    };
    let step: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT step FROM undo_journal WHERE stack = ?1 ORDER BY step {order} LIMIT 1"
            ),
            [from],
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
        stmt.query_map(params![from, step], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    tx.execute(
        "DELETE FROM undo_journal WHERE stack = ?1 AND step = ?2",
        params![from, step],
    )?;
    // 実行中の変更は反対側の履歴に記録される（undo 中 → redo へ、redo 中 → undo へ）
    tx.execute(
        "UPDATE undo_state SET mode = ?1, step = ?2 WHERE id = 1",
        params![from.applying_mode(), step],
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

    /// 画面と MCP のように、別々の接続が同じファイルに同時に書いても失敗しない
    #[test]
    fn concurrent_writers_on_same_file_do_not_fail() {
        let path =
            std::env::temp_dir().join(format!("my_todo_concurrency_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let setup = db::open(&path).unwrap();
        let a = add(&setup, "a", None);
        drop(setup);
        let writers: Vec<_> = (0..2)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let conn = db::open(&path).unwrap();
                    for i in 0..50 {
                        let patch = TaskPatch {
                            title: Some(format!("w{w}-{i}")),
                            ..Default::default()
                        };
                        record(&conn, "変更", |c| ts::update_task(c, a.id, patch)).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().expect("書き込みが失敗した");
        }
        let conn = db::open(&path).unwrap();
        let steps: i64 = conn
            .query_row("SELECT COUNT(*) FROM undo_step", [], |r| r.get(0))
            .unwrap();
        assert_eq!(steps, 101); // 追加 1 + 変更 100
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
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
