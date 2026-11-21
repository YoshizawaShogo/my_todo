//! Jira 連携のうち、通信に依存しない部分：設定、送る内容の下書き、送った記録。
//!
//! 送る単位は「日ごと・課題ごと」。工数を記録したタスクから、課題キーが紐付いた一番近い
//! 祖先（自分を含む）をたどり、その課題に合算する。アプリ側でタスクを細かく分けても、
//! Jira には紐付けた課題の単位でまとめて送れる。
//! 送った worklog は jira_worklog に覚えておき、差分だけを送る（新規・更新・削除）。

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, params};
use serde::Serialize;

use super::{ServiceError, ServiceResult, SettingKey, now_text, task_service};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JiraConfig {
    /// 例：https://jira.example.co.jp（末尾の / は除く）
    pub base_url: Option<String>,
}

pub fn get_config(conn: &Connection) -> ServiceResult<JiraConfig> {
    Ok(JiraConfig {
        base_url: super::get_setting(conn, SettingKey::JiraBaseUrl)?.filter(|s| !s.is_empty()),
    })
}

pub fn set_base_url(conn: &Connection, url: &str) -> ServiceResult<JiraConfig> {
    let url = url.trim().trim_end_matches('/');
    // PAT を平文で流さないよう、http:// はこの PC の中（localhost・127.0.0.1）だけに限る
    let local_http = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split(['/', ':']).next())
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1"));
    if !(url.is_empty() || url.starts_with("https://") || local_http) {
        return Err(ServiceError::Invalid(
            "Jira の URL は https:// から始めてください（PAT が暗号化されずに流れるのを防ぐため）"
                .into(),
        ));
    }
    super::set_setting(conn, SettingKey::JiraBaseUrl, url)?;
    get_config(conn)
}

str_enum! {
    /// 下書きの各行で Jira に対して行うこと
    pub enum DraftAction {
        /// まだ送っていない → 新しく作る
        Create => "create",
        /// 送った時間と違う → 更新する
        Update => "update",
        /// 送ったが今は 0 → 削除する
        Delete => "delete",
        /// 送った時間と同じ
        Unchanged => "unchanged",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DraftLine {
    pub work_date: String,
    pub issue_key: String,
    /// 送る時間（分）
    pub minutes: i64,
    /// 送信済みの時間（分）
    pub sent_minutes: Option<i64>,
    pub worklog_id: Option<String>,
    pub action: DraftAction,
    /// この行にまとめたタスク（"親 › 子" の形）
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnlinkedLine {
    pub work_date: String,
    pub task_id: i64,
    pub task: String,
    pub minutes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Draft {
    pub from: String,
    pub to: String,
    pub lines: Vec<DraftLine>,
    /// 課題キーが自分にも祖先にもないため送れない工数
    pub unlinked: Vec<UnlinkedLine>,
}

fn check_date(d: &str) -> ServiceResult<()> {
    super::parse_date(d).map(|_| ())
}

/// 期間内の工数から、Jira に送る内容の下書きを作る
pub fn build_draft(conn: &Connection, from: &str, to: &str) -> ServiceResult<Draft> {
    check_date(from)?;
    check_date(to)?;
    let tasks = task_service::list_tasks(conn)?;
    let by_id: HashMap<i64, &crate::model::Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let path = |id: i64| {
        let mut names = Vec::new();
        let mut cur = by_id.get(&id).copied();
        while let Some(t) = cur {
            names.push(t.title.clone());
            cur = t.parent_id.and_then(|p| by_id.get(&p).copied());
        }
        names.reverse();
        names.join(" › ")
    };
    // 自分か祖先のうち、一番近い紐付け
    let linked_key = |id: i64| {
        let mut cur = by_id.get(&id).copied();
        while let Some(t) = cur {
            if let Some(k) = &t.jira_key {
                return Some(k.clone());
            }
            cur = t.parent_id.and_then(|p| by_id.get(&p).copied());
        }
        None
    };

    // (日付, 課題) → (分, タスク名)
    let mut want: BTreeMap<(String, String), (i64, Vec<String>)> = BTreeMap::new();
    let mut unlinked = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT work_date, task_id, SUM(minutes) FROM time_entry \
         WHERE work_date BETWEEN ?1 AND ?2 GROUP BY work_date, task_id \
         HAVING SUM(minutes) > 0 ORDER BY work_date, task_id",
    )?;
    let rows = stmt
        .query_map([from, to], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (date, task_id, minutes) in rows {
        match linked_key(task_id) {
            Some(key) => {
                let e = want.entry((date, key)).or_insert((0, Vec::new()));
                e.0 += minutes;
                e.1.push(path(task_id));
            }
            None => unlinked.push(UnlinkedLine {
                work_date: date,
                task_id,
                task: path(task_id),
                minutes,
            }),
        }
    }

    // 送信済み
    let mut sent: BTreeMap<(String, String), (i64, String)> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT work_date, issue_key, minutes, worklog_id FROM jira_worklog \
         WHERE work_date BETWEEN ?1 AND ?2",
    )?;
    for row in stmt.query_map([from, to], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (d, k, m, w) = row?;
        sent.insert((d, k), (m, w));
    }

    let mut keys: Vec<(String, String)> = want.keys().chain(sent.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    let lines = keys
        .into_iter()
        .map(|key| {
            let (minutes, tasks) = want.get(&key).cloned().unwrap_or((0, Vec::new()));
            let s = sent.get(&key);
            let action = match (minutes, s) {
                (0, Some(_)) => DraftAction::Delete,
                (_, None) => DraftAction::Create,
                (m, Some((sm, _))) if m == *sm => DraftAction::Unchanged,
                _ => DraftAction::Update,
            };
            DraftLine {
                work_date: key.0,
                issue_key: key.1,
                minutes,
                sent_minutes: s.map(|x| x.0),
                worklog_id: s.map(|x| x.1.clone()),
                action,
                tasks,
            }
        })
        .collect();
    Ok(Draft {
        from: from.into(),
        to: to.into(),
        lines,
        unlinked,
    })
}

/// 送った結果を覚える（新規・更新）
pub fn record_sent(
    conn: &Connection,
    work_date: &str,
    issue_key: &str,
    worklog_id: &str,
    minutes: i64,
) -> ServiceResult<()> {
    conn.execute(
        "INSERT INTO jira_worklog (work_date, issue_key, worklog_id, minutes, sent_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(work_date, issue_key) DO UPDATE SET \
           worklog_id = excluded.worklog_id, minutes = excluded.minutes, sent_at = excluded.sent_at",
        params![work_date, issue_key, worklog_id, minutes, now_text()],
    )?;
    Ok(())
}

/// Jira 側で削除したので記録も消す
pub fn forget_sent(conn: &Connection, work_date: &str, issue_key: &str) -> ServiceResult<()> {
    conn.execute(
        "DELETE FROM jira_worklog WHERE work_date = ?1 AND issue_key = ?2",
        params![work_date, issue_key],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{NewTask, TaskPatch};
    use crate::service::work_service::{Allocation, SaveDay, save_day};

    fn add(conn: &Connection, title: &str, parent_id: Option<i64>, key: Option<&str>) -> i64 {
        let t = task_service::create_task(
            conn,
            NewTask {
                title: title.into(),
                parent_id,
                ..Default::default()
            },
        )
        .unwrap();
        if let Some(k) = key {
            let p: TaskPatch =
                serde_json::from_value(serde_json::json!({ "jira_key": k })).unwrap();
            task_service::update_task(conn, t.id, p).unwrap();
        }
        t.id
    }

    fn day(conn: &Connection, date: &str, worked: i64, alloc: &[(i64, f64)]) {
        save_day(
            conn,
            date,
            SaveDay {
                worked_min: worked,
                allocations: alloc
                    .iter()
                    .map(|&(task_id, weight)| Allocation { task_id, weight })
                    .collect(),
            },
        )
        .unwrap();
    }

    #[test]
    fn rolls_up_to_nearest_linked_ancestor_and_diffs_with_sent() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "提案", None, Some("ABC-1"));
        let c1 = add(&conn, "構成", Some(p), None);
        let c2 = add(&conn, "レビュー", Some(p), Some("ABC-2"));
        let other = add(&conn, "雑務", None, None);
        day(
            &conn,
            "2026-11-20",
            600,
            &[(c1, 2.0), (c2, 1.0), (p, 1.0), (other, 1.0)],
        );
        // 10h を 2:1:1:1 → 4h / 2h / 2h / 2h

        let d = build_draft(&conn, "2026-11-16", "2026-11-22").unwrap();
        let lines: Vec<_> = d
            .lines
            .iter()
            .map(|l| (l.issue_key.as_str(), l.minutes, l.action))
            .collect();
        assert_eq!(
            lines,
            [
                ("ABC-1", 360, DraftAction::Create),
                ("ABC-2", 120, DraftAction::Create)
            ]
        );
        assert_eq!(d.lines[0].tasks, ["提案", "提案 › 構成"]);
        assert_eq!(d.unlinked.len(), 1);
        assert_eq!(
            (d.unlinked[0].task.as_str(), d.unlinked[0].minutes),
            ("雑務", 120)
        );

        // 送った後に工数を変える → 更新・削除・変化なし
        record_sent(&conn, "2026-11-20", "ABC-1", "w1", 360).unwrap();
        record_sent(&conn, "2026-11-20", "ABC-2", "w2", 120).unwrap();
        record_sent(&conn, "2026-11-19", "ABC-9", "w9", 60).unwrap();
        day(&conn, "2026-11-20", 600, &[(c1, 2.0), (c2, 1.0), (p, 1.0)]);
        // 10h を 2:1:1 → 5h / 2.5h / 2.5h → ABC-1 は 7.5h、ABC-2 は 2.5h
        let d = build_draft(&conn, "2026-11-16", "2026-11-22").unwrap();
        let lines: Vec<_> = d
            .lines
            .iter()
            .map(|l| {
                (
                    l.work_date.as_str(),
                    l.issue_key.as_str(),
                    l.minutes,
                    l.action,
                    l.worklog_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                ("2026-11-19", "ABC-9", 0, DraftAction::Delete, Some("w9")),
                ("2026-11-20", "ABC-1", 450, DraftAction::Update, Some("w1")),
                ("2026-11-20", "ABC-2", 150, DraftAction::Update, Some("w2")),
            ]
        );
        record_sent(&conn, "2026-11-20", "ABC-1", "w1", 450).unwrap();
        forget_sent(&conn, "2026-11-19", "ABC-9").unwrap();
        let d = build_draft(&conn, "2026-11-16", "2026-11-22").unwrap();
        assert_eq!(d.lines[0].action, DraftAction::Unchanged);
        assert_eq!(d.lines.len(), 2);
    }

    #[test]
    fn base_url_is_validated() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(get_config(&conn).unwrap().base_url, None);
        let c = set_base_url(&conn, " https://jira.example.com/ ").unwrap();
        assert_eq!(c.base_url.as_deref(), Some("https://jira.example.com"));
        assert!(set_base_url(&conn, "jira.example.com").is_err());
        assert!(set_base_url(&conn, "http://jira.example.com").is_err());
        assert!(set_base_url(&conn, "http://127.0.0.1:8798").is_ok());
    }
}
