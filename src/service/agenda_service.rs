//! 「今日・今週やること」の一覧。次の予定・終了予定日・作業中のタスクから組み立てる。

use std::collections::HashMap;

use jiff::ToSpan;
use jiff::civil::Date;
use rusqlite::Connection;
use serde::Serialize;

use super::ServiceResult;
use super::task_service;
use crate::model::{Task, TaskStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgendaKind {
    /// 次の予定（next_action_date）
    NextAction,
    /// 終了予定日（planned_end）
    PlannedEnd,
    /// 日付はないが作業中
    Doing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgendaItem {
    pub task_id: i64,
    pub title: String,
    /// 親から順に並べた祖先のタイトル
    pub path: Vec<String>,
    pub kind: AgendaKind,
    pub date: Option<String>,
    /// 次の予定の内容（kind = next_action のとき）
    pub text: Option<String>,
    pub status: TaskStatus,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agenda {
    pub today: String,
    /// 今週の最終日（日曜）
    pub week_end: String,
    /// 来週の最終日（日曜）
    pub next_week_end: String,
    pub overdue: Vec<AgendaItem>,
    pub today_items: Vec<AgendaItem>,
    /// 明日から今週末まで
    pub this_week: Vec<AgendaItem>,
    pub next_week: Vec<AgendaItem>,
}

fn parse_date(s: &Option<String>) -> Option<Date> {
    s.as_deref().and_then(|d| super::parse_date(d).ok())
}

fn ymd(d: Date) -> String {
    d.to_string()
}

/// 週は月曜始まり・日曜終わり
fn week_end_of(d: Date) -> Date {
    add_days_saturating(d, 6 - i64::from(d.weekday().to_monday_zero_offset()))
}

/// 9999-12-31 を超えるときは 9999-12-31 で止める（パニックさせない）
fn add_days_saturating(d: Date, n: i64) -> Date {
    d.checked_add(n.days()).unwrap_or(Date::MAX)
}

pub fn agenda(conn: &Connection, today: Date) -> ServiceResult<Agenda> {
    let tasks = task_service::list_tasks(conn)?;
    Ok(build_agenda(&tasks, today))
}

pub fn build_agenda(tasks: &[Task], today: Date) -> Agenda {
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let path_of = |task: &Task| {
        let mut path = Vec::new();
        let mut cur = task.parent_id.and_then(|p| by_id.get(&p));
        while let Some(t) = cur {
            path.push(t.title.clone());
            cur = t.parent_id.and_then(|p| by_id.get(&p));
        }
        path.reverse();
        path
    };

    let week_end = week_end_of(today);
    let next_week_end = add_days_saturating(week_end, 7);
    let mut agenda = Agenda {
        today: ymd(today),
        week_end: ymd(week_end),
        next_week_end: ymd(next_week_end),
        overdue: Vec::new(),
        today_items: Vec::new(),
        this_week: Vec::new(),
        next_week: Vec::new(),
    };

    for task in tasks.iter().filter(|t| t.status != TaskStatus::Done) {
        let item = |kind, date: Option<Date>, text: Option<String>| AgendaItem {
            task_id: task.id,
            title: task.title.clone(),
            path: path_of(task),
            kind,
            date: date.map(ymd),
            text,
            status: task.status,
            tags: task.tags.clone(),
        };
        let mut shown_by_today = false;
        let candidates = [
            (
                AgendaKind::NextAction,
                parse_date(&task.next_action_date),
                task.next_action.clone(),
            ),
            (AgendaKind::PlannedEnd, parse_date(&task.planned_end), None),
        ];
        for (kind, date, text) in candidates {
            let Some(d) = date else { continue };
            let bucket = if d < today {
                &mut agenda.overdue
            } else if d == today {
                &mut agenda.today_items
            } else if d <= week_end {
                &mut agenda.this_week
            } else if d <= next_week_end {
                &mut agenda.next_week
            } else {
                continue;
            };
            shown_by_today |= d <= today;
            bucket.push(item(kind, date, text));
        }
        // 作業中なのに今日までの予定がないものは「今日」に出して忘れないようにする
        if task.status == TaskStatus::Doing && !shown_by_today {
            agenda.today_items.push(item(AgendaKind::Doing, None, None));
        }
    }

    // 日付順。同じ日なら次の予定 → 終了予定日 → 作業中
    let order = |k: AgendaKind| match k {
        AgendaKind::NextAction => 0,
        AgendaKind::PlannedEnd => 1,
        AgendaKind::Doing => 2,
    };
    for list in [
        &mut agenda.overdue,
        &mut agenda.today_items,
        &mut agenda.this_week,
        &mut agenda.next_week,
    ] {
        list.sort_by(|a, b| {
            (a.date.is_none(), &a.date, order(a.kind)).cmp(&(
                b.date.is_none(),
                &b.date,
                order(b.kind),
            ))
        });
    }
    agenda
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{NewTask, TaskPatch};

    fn add(conn: &Connection, title: &str, parent_id: Option<i64>, patch: &str) -> i64 {
        let t = task_service::create_task(
            conn,
            NewTask {
                title: title.into(),
                parent_id,
                ..Default::default()
            },
        )
        .unwrap();
        let patch: TaskPatch = serde_json::from_str(patch).unwrap();
        task_service::update_task(conn, t.id, patch).unwrap();
        t.id
    }

    fn titles(items: &[AgendaItem]) -> Vec<(String, AgendaKind)> {
        items.iter().map(|i| (i.title.clone(), i.kind)).collect()
    }

    #[test]
    fn week_ends_on_sunday() {
        let d = |s| super::super::parse_date(s).unwrap();
        assert_eq!(week_end_of(d("2026-11-20")), d("2026-11-22")); // 金 → 日
        assert_eq!(week_end_of(d("2026-11-22")), d("2026-11-22")); // 日 → 同じ日
        assert_eq!(week_end_of(d("2026-11-23")), d("2026-11-29")); // 月 → 次の日
    }

    #[test]
    fn items_are_bucketed_by_date() {
        let conn = db::open_in_memory().unwrap();
        let today = jiff::civil::date(2026, 11, 20); // 金曜
        let p = add(&conn, "資料作成", None, r#"{"planned_end":"2026-11-27"}"#);
        add(
            &conn,
            "レビュー",
            Some(p),
            r#"{"next_action_date":"2026-11-20","next_action":"依頼する"}"#,
        );
        add(
            &conn,
            "下書き",
            Some(p),
            r#"{"next_action_date":"2026-11-19"}"#,
        );
        add(&conn, "週末作業", None, r#"{"planned_end":"2026-11-22"}"#);
        add(&conn, "作業中のもの", None, r#"{"status":"doing"}"#);
        add(
            &conn,
            "完了済み",
            None,
            r#"{"status":"done","planned_end":"2026-11-20"}"#,
        );
        add(&conn, "遠い先", None, r#"{"planned_end":"2026-12-31"}"#);

        let a = agenda(&conn, today).unwrap();
        assert_eq!(a.week_end, "2026-11-22");
        assert_eq!(a.next_week_end, "2026-11-29");
        assert_eq!(
            titles(&a.overdue),
            [("下書き".into(), AgendaKind::NextAction)]
        );
        assert_eq!(
            titles(&a.today_items),
            [
                ("レビュー".into(), AgendaKind::NextAction),
                ("作業中のもの".into(), AgendaKind::Doing),
            ]
        );
        assert_eq!(a.today_items[0].path, ["資料作成"]);
        assert_eq!(a.today_items[0].text.as_deref(), Some("依頼する"));
        assert_eq!(
            titles(&a.this_week),
            [("週末作業".into(), AgendaKind::PlannedEnd)]
        );
        assert_eq!(
            titles(&a.next_week),
            [("資料作成".into(), AgendaKind::PlannedEnd)]
        );
    }

    #[test]
    fn doing_task_with_today_item_is_not_duplicated() {
        let conn = db::open_in_memory().unwrap();
        let today = jiff::civil::date(2026, 11, 20);
        add(
            &conn,
            "a",
            None,
            r#"{"status":"doing","next_action_date":"2026-11-20"}"#,
        );
        let a = agenda(&conn, today).unwrap();
        assert_eq!(
            titles(&a.today_items),
            [("a".into(), AgendaKind::NextAction)]
        );
    }
}
