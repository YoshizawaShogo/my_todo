//! 振り返り：完了したタスクの見積もりと実績を並べ、ずれの理由と自由記述を残す。
//! 日々の操作には何も足さず、時間のあるときに振り返りのタブでまとめて書く。

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::{ServiceError, ServiceResult, Tx, now_text, parse_date, task_service};
use crate::model::{EventKind, Task, TaskKind, TaskStatus};

str_enum! {
    /// 見積もりと実績のずれの理由（迷わないよう、よくあるものだけに絞る）
    pub enum ReviewReason {
        /// 見積もりどおり
        AsEstimated => "as_estimated",
        /// 想定より簡単だった
        Easier => "easier",
        /// 調査・確認に時間がかかった
        Investigation => "investigation",
        /// 仕様が変わった
        SpecChange => "spec_change",
        /// 手戻りがあった
        Rework => "rework",
        /// 割り込み・待ちがあった
        Interruption => "interruption",
        /// 作業が見積もりから漏れていた
        Missed => "missed",
    }
}

/// 内訳：子タスクごとの見積もり（最初の値。孫があれば合計）と実績（孫を含む）
#[derive(Debug, Serialize)]
pub struct Breakdown {
    pub task_id: i64,
    pub title: String,
    pub estimate_min: Option<i64>,
    pub actual_min: i64,
}

#[derive(Debug, Serialize)]
pub struct ReviewItem {
    pub task_id: i64,
    pub title: String,
    /// 親の名前（"親 › 子" の形）
    pub path: Vec<String>,
    pub completed_on: String,
    /// 最初に入れた見積もり（分）。変更の履歴から取る。親に入れていなければ、子の見積もりの合計
    pub first_estimate_min: Option<i64>,
    /// first_estimate_min を子の見積もりの合計から出したか
    pub estimate_from_children: bool,
    /// 親に直接付けた実績（分）。子があるときは内訳の 1 つとして出す
    pub own_actual_min: i64,
    /// 今の見積もり（分）
    pub estimate_min: Option<i64>,
    /// 実績（分）。子タスクの分も含む
    pub actual_min: i64,
    pub breakdown: Vec<Breakdown>,
    pub reasons: Vec<ReviewReason>,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewInput {
    #[serde(default)]
    pub reasons: Vec<ReviewReason>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 期間内（完了日が from〜to）に完了した自分のタスクの振り返り。新しい順
pub fn list(conn: &Connection, from: &str, to: &str) -> ServiceResult<Vec<ReviewItem>> {
    parse_date(from)?;
    parse_date(to)?;
    let tasks = task_service::list_tasks(conn)?;
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut children: HashMap<i64, Vec<&Task>> = HashMap::new();
    for t in &tasks {
        if let Some(p) = t.parent_id {
            children.entry(p).or_default().push(t);
        }
    }
    fn subtree_actual(t: &Task, children: &HashMap<i64, Vec<&Task>>) -> i64 {
        t.actual_min
            + children
                .get(&t.id)
                .map(|cs| cs.iter().map(|c| subtree_actual(c, children)).sum())
                .unwrap_or(0)
    }
    // 最初の見積もり：見積もりの変更の履歴で一番古いものの「変更後」
    let mut first_estimate: HashMap<i64, Option<i64>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT task_id, new_value FROM task_event WHERE kind = ?1 ORDER BY at, id")?;
        for row in stmt.query_map([EventKind::Estimate], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })? {
            let (id, v) = row?;
            first_estimate
                .entry(id)
                .or_insert_with(|| v.and_then(|s| s.parse().ok()));
        }
    }
    // 見積もり：自分の最初の見積もり。なければ子の見積もりの合計（子にもなければ None）
    fn subtree_estimate(
        t: &Task,
        children: &HashMap<i64, Vec<&Task>>,
        first: &HashMap<i64, Option<i64>>,
    ) -> Option<i64> {
        let own = first.get(&t.id).copied().flatten().or(t.estimate_min);
        if own.is_some() {
            return own;
        }
        let kids: Vec<i64> = children
            .get(&t.id)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| subtree_estimate(c, children, first))
                    .collect()
            })
            .unwrap_or_default();
        (!kids.is_empty()).then(|| kids.iter().sum())
    }
    let mut reviews: HashMap<i64, (Vec<ReviewReason>, Option<String>)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT task_id, reasons, note FROM task_review")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })? {
            let (id, reasons, note) = row?;
            reviews.insert(
                id,
                (serde_json::from_str(&reasons).unwrap_or_default(), note),
            );
        }
    }
    let mut items: Vec<ReviewItem> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done && t.kind == TaskKind::Own)
        .filter_map(|t| {
            let day = t.completed_at.as_deref()?.get(..10)?.to_string();
            (from <= day.as_str() && day.as_str() <= to).then_some((t, day))
        })
        .map(|(t, day)| {
            let mut path = Vec::new();
            let mut cur = t.parent_id.and_then(|p| by_id.get(&p));
            while let Some(p) = cur {
                path.push(p.title.clone());
                cur = p.parent_id.and_then(|q| by_id.get(&q));
            }
            path.reverse();
            let (reasons, note) = reviews.remove(&t.id).unwrap_or_default();
            let own_first = first_estimate
                .get(&t.id)
                .copied()
                .flatten()
                .or(t.estimate_min);
            let total_estimate = subtree_estimate(t, &children, &first_estimate);
            ReviewItem {
                task_id: t.id,
                title: t.title.clone(),
                path,
                completed_on: day,
                first_estimate_min: total_estimate,
                estimate_from_children: own_first.is_none() && total_estimate.is_some(),
                own_actual_min: t.actual_min,
                estimate_min: t.estimate_min,
                actual_min: subtree_actual(t, &children),
                breakdown: children
                    .get(&t.id)
                    .map(|cs| {
                        cs.iter()
                            .map(|c| Breakdown {
                                task_id: c.id,
                                title: c.title.clone(),
                                estimate_min: subtree_estimate(c, &children, &first_estimate),
                                actual_min: subtree_actual(c, &children),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                reasons,
                note,
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.completed_on
            .cmp(&a.completed_on)
            .then(b.task_id.cmp(&a.task_id))
    });
    Ok(items)
}

/// 振り返りを保存する（理由も記述も空なら消す）
pub fn save(conn: &Connection, task_id: i64, input: ReviewInput) -> ServiceResult<()> {
    let note = input
        .note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let mut reasons = input.reasons;
    reasons.sort_by_key(|r| ReviewReason::ALL.iter().position(|x| x == r));
    reasons.dedup();
    let tx = Tx::begin(conn)?;
    task_service::get_task(&tx, task_id)?;
    if reasons.is_empty() && note.is_none() {
        tx.execute("DELETE FROM task_review WHERE task_id = ?1", [task_id])?;
    } else {
        let json = serde_json::to_string(&reasons)
            .map_err(|e| ServiceError::Invalid(format!("理由を保存できません: {e}")))?;
        tx.execute(
            "INSERT INTO task_review (task_id, reasons, note, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(task_id) DO UPDATE SET reasons = excluded.reasons, note = excluded.note, \
             updated_at = excluded.updated_at",
            params![task_id, json, note, now_text()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 1 件の振り返り（テスト用）
#[cfg(test)]
fn get(conn: &Connection, task_id: i64) -> ServiceResult<Option<(String, Option<String>)>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT reasons, note FROM task_review WHERE task_id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{NewTask, TaskPatch};
    use crate::service::undo_service;
    use crate::service::work_service::{Allocation, SaveDay, save_day};

    fn add(conn: &Connection, title: &str, parent_id: Option<i64>) -> i64 {
        task_service::create_task(
            conn,
            NewTask {
                title: title.into(),
                parent_id,
                inbox: Some(false),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn patch(conn: &Connection, id: i64, json: &str) {
        let p: TaskPatch = serde_json::from_str(json).unwrap();
        task_service::update_task(conn, id, p).unwrap();
    }

    /// 親に見積もりがなければ子の見積もりの合計。内訳に子ごとの見積もりと実績
    #[test]
    fn parent_estimate_is_sum_of_children() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None);
        let a = add(&conn, "調査", Some(p));
        let b = add(&conn, "実装", Some(p));
        let c = add(&conn, "後から", Some(p));
        patch(&conn, a, r#"{"estimate_min":60}"#);
        patch(&conn, b, r#"{"estimate_min":120}"#);
        save_day(
            &conn,
            "2026-11-20",
            SaveDay {
                worked_min: 480,
                allocations: vec![
                    Allocation {
                        task_id: a,
                        weight: 1.0,
                    },
                    Allocation {
                        task_id: b,
                        weight: 2.0,
                    },
                    Allocation {
                        task_id: c,
                        weight: 1.0,
                    },
                ],
            },
        )
        .unwrap();
        patch(&conn, p, r#"{"status":"done"}"#);
        let today = crate::service::today().to_string();
        let it = list(&conn, "2000-01-01", &today)
            .unwrap()
            .into_iter()
            .find(|i| i.task_id == p)
            .unwrap();
        assert_eq!(it.first_estimate_min, Some(180));
        assert!(it.estimate_from_children);
        assert_eq!(it.actual_min, 480);
        let b: Vec<(Option<i64>, i64)> = it
            .breakdown
            .iter()
            .map(|x| (x.estimate_min, x.actual_min))
            .collect();
        assert_eq!(b, [(Some(60), 120), (Some(120), 240), (None, 120)]);
    }

    #[test]
    fn lists_estimate_actual_breakdown_and_review() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "機能 X", None);
        let c1 = add(&conn, "調査", Some(p));
        let c2 = add(&conn, "実装", Some(p));
        patch(&conn, p, r#"{"estimate_min":240}"#);
        // 5 分以上あけないと見積もりの変更は 1 件にまとまるので、履歴の時刻をずらしておく
        conn.execute(
            "UPDATE task_event SET at = '2000-01-01T00:00:00+09:00' WHERE kind = 'estimate'",
            [],
        )
        .unwrap();
        patch(&conn, p, r#"{"estimate_min":360}"#);
        save_day(
            &conn,
            "2026-11-20",
            SaveDay {
                worked_min: 480,
                allocations: vec![
                    Allocation {
                        task_id: c1,
                        weight: 1.0,
                    },
                    Allocation {
                        task_id: c2,
                        weight: 3.0,
                    },
                ],
            },
        )
        .unwrap();
        patch(&conn, p, r#"{"status":"done"}"#);
        let today = crate::service::today().to_string();
        let items = list(&conn, "2000-01-01", &today).unwrap();
        let it = items.iter().find(|i| i.task_id == p).unwrap();
        assert_eq!(it.first_estimate_min, Some(240));
        assert_eq!(it.estimate_min, Some(360));
        assert_eq!(it.actual_min, 480);
        let b: Vec<(String, i64)> = it
            .breakdown
            .iter()
            .map(|b| (b.title.clone(), b.actual_min))
            .collect();
        assert_eq!(b, [("調査".to_string(), 120), ("実装".to_string(), 360)]);

        undo_service::record(&conn, "振り返り", |c| {
            save(
                c,
                p,
                ReviewInput {
                    reasons: vec![
                        ReviewReason::Rework,
                        ReviewReason::Investigation,
                        ReviewReason::Rework,
                    ],
                    note: Some("  仕様の確認待ち  ".into()),
                },
            )
        })
        .unwrap();
        let it = list(&conn, "2000-01-01", &today)
            .unwrap()
            .into_iter()
            .find(|i| i.task_id == p)
            .unwrap();
        assert_eq!(
            it.reasons,
            [ReviewReason::Investigation, ReviewReason::Rework]
        );
        assert_eq!(it.note.as_deref(), Some("仕様の確認待ち"));
        undo_service::undo(&conn).unwrap();
        assert_eq!(get(&conn, p).unwrap(), None);
        // 空で保存すると消える
        save(
            &conn,
            p,
            ReviewInput {
                reasons: vec![ReviewReason::Easier],
                note: None,
            },
        )
        .unwrap();
        save(
            &conn,
            p,
            ReviewInput {
                reasons: vec![],
                note: Some(" ".into()),
            },
        )
        .unwrap();
        assert_eq!(get(&conn, p).unwrap(), None);
        // 期間外は出ない・他の人の作業は出ない
        assert!(list(&conn, "1999-01-01", "1999-12-31").unwrap().is_empty());
        let other = add(&conn, "他の人", None);
        patch(&conn, other, r#"{"kind":"external","status":"done"}"#);
        assert!(
            list(&conn, "2000-01-01", &today)
                .unwrap()
                .iter()
                .all(|i| i.task_id != other)
        );
    }
}
