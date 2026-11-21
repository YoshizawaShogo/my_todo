use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{ServiceError, ServiceResult, Tx, now_text};
use crate::model::{
    EventKind, MoveTarget, NewTask, Placement, TagCount, Task, TaskEvent, TaskPatch, TaskStatus,
};

const TASK_COLUMNS: &str = "id, parent_id, title, kind, external_job_id, status, is_inbox, \
     sort_order, planned_start, planned_end, estimate_min, note, created_at, updated_at, completed_at, \
     started_at, next_action, next_action_date";

fn row_to_task(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        kind: row.get(3)?,
        external_job_id: row.get(4)?,
        status: row.get(5)?,
        is_inbox: row.get(6)?,
        sort_order: row.get(7)?,
        planned_start: row.get(8)?,
        planned_end: row.get(9)?,
        estimate_min: row.get(10)?,
        note: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        completed_at: row.get(14)?,
        started_at: row.get(15)?,
        next_action: row.get(16)?,
        next_action_date: row.get(17)?,
        tags: Vec::new(),
        actual_min: 0,
        jira_key: None,
    })
}

fn normalize_title(title: &str) -> ServiceResult<String> {
    let t = title.trim();
    if t.is_empty() {
        return Err(ServiceError::Invalid("タイトルは必須です".into()));
    }
    Ok(t.to_string())
}

fn normalize_note(note: Option<String>) -> Option<String> {
    note.filter(|n| !n.trim().is_empty())
}

fn normalize_text(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// 日付は YYYY-MM-DD。空文字は「未設定」として扱う。
fn normalize_date(date: Option<String>) -> ServiceResult<Option<String>> {
    match normalize_text(date) {
        None => Ok(None),
        Some(d) => super::parse_date(&d).map(|v| Some(v.to_string())),
    }
}

/// Jira の課題キーは「英大文字で始まるプロジェクトキー - 番号」（例：ABC-123）。小文字は大文字に直す
pub fn normalize_issue_key(key: Option<String>) -> ServiceResult<Option<String>> {
    let Some(k) = normalize_text(key) else {
        return Ok(None);
    };
    let k = k.to_uppercase();
    let valid = match k.split_once('-') {
        Some((project, num)) => {
            project
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
                && project
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    };
    if !valid {
        return Err(ServiceError::Invalid(format!(
            "Jira の課題キーは ABC-123 のような形で入れてください: {k}"
        )));
    }
    Ok(Some(k))
}

/// 次の予定を履歴用に 1 つの文字列にまとめる
fn next_action_text(date: &Option<String>, text: &Option<String>) -> Option<String> {
    match (date, text) {
        (None, None) => None,
        (Some(d), None) => Some(d.clone()),
        (None, Some(t)) => Some(t.clone()),
        (Some(d), Some(t)) => Some(format!("{d} {t}")),
    }
}

/// 同じ項目をこの時間内に変え直したら、直前の履歴にまとめる（入力途中の細かい変更を残さない）
const EVENT_MERGE_SECONDS: i64 = 5 * 60;

fn record_event(
    conn: &Connection,
    task_id: i64,
    at: &str,
    kind: EventKind,
    old_value: Option<&str>,
    new_value: Option<&str>,
) -> ServiceResult<()> {
    if kind != EventKind::Created {
        let last: Option<(i64, String, EventKind, Option<String>)> = conn
            .query_row(
                "SELECT id, at, kind, old_value FROM task_event WHERE task_id = ?1 \
                 ORDER BY at DESC, id DESC LIMIT 1",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((last_id, last_at, last_kind, first_old)) = last {
            let recent = match (
                last_at.parse::<jiff::Timestamp>(),
                at.parse::<jiff::Timestamp>(),
            ) {
                (Ok(a), Ok(b)) => b.duration_since(a).as_secs() <= EVENT_MERGE_SECONDS,
                _ => false,
            };
            if recent && last_kind == kind {
                if first_old.as_deref() == new_value {
                    // 結局元に戻した
                    conn.execute("DELETE FROM task_event WHERE id = ?1", [last_id])?;
                } else {
                    conn.execute(
                        "UPDATE task_event SET at = ?1, new_value = ?2 WHERE id = ?3",
                        params![at, new_value, last_id],
                    )?;
                }
                return Ok(());
            }
        }
    }
    conn.execute(
        "INSERT INTO task_event (task_id, at, kind, old_value, new_value) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, at, kind, old_value, new_value],
    )?;
    Ok(())
}

/// タスクの変更履歴を古い順に返す
pub fn list_events(conn: &Connection, task_id: i64) -> ServiceResult<Vec<TaskEvent>> {
    get_task(conn, task_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, at, kind, old_value, new_value FROM task_event \
         WHERE task_id = ?1 ORDER BY at, id",
    )?;
    let events = stmt
        .query_map([task_id], |r| {
            Ok(TaskEvent {
                id: r.get(0)?,
                task_id: r.get(1)?,
                at: r.get(2)?,
                kind: r.get(3)?,
                old_value: r.get(4)?,
                new_value: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(events)
}

/// 兄弟の集合。最上位は未分類とタスク一覧に分かれる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Siblings {
    Child(i64),
    Root { inbox: bool },
}

impl Siblings {
    fn of(task: &Task) -> Self {
        match task.parent_id {
            Some(p) => Siblings::Child(p),
            None => Siblings::Root {
                inbox: task.is_inbox,
            },
        }
    }

    /// sort_order 順の兄弟 id 一覧
    pub(crate) fn ids(self, conn: &Connection) -> ServiceResult<Vec<i64>> {
        let mut ids = Vec::new();
        match self {
            Siblings::Child(p) => {
                let mut stmt = conn
                    .prepare("SELECT id FROM task WHERE parent_id = ?1 ORDER BY sort_order, id")?;
                for id in stmt.query_map([p], |r| r.get(0))? {
                    ids.push(id?);
                }
            }
            Siblings::Root { inbox } => {
                let mut stmt = conn.prepare(
                    "SELECT id FROM task WHERE parent_id IS NULL AND is_inbox = ?1 \
                     ORDER BY sort_order, id",
                )?;
                for id in stmt.query_map([inbox], |r| r.get(0))? {
                    ids.push(id?);
                }
            }
        }
        Ok(ids)
    }
}

fn renumber(conn: &Connection, ids: &[i64]) -> ServiceResult<()> {
    let mut stmt = conn.prepare("UPDATE task SET sort_order = ?1 WHERE id = ?2")?;
    for (i, id) in ids.iter().enumerate() {
        stmt.execute(params![i as i64, id])?;
    }
    Ok(())
}

pub fn get_task(conn: &Connection, id: i64) -> ServiceResult<Task> {
    let mut task = conn
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?1"),
            [id],
            row_to_task,
        )
        .optional()?
        .ok_or(ServiceError::NotFound(id))?;
    task.tags = tags_of(conn, id)?;
    task.actual_min = conn.query_row(
        "SELECT COALESCE(SUM(minutes), 0) FROM time_entry WHERE task_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    task.jira_key = conn
        .query_row(
            "SELECT issue_key FROM jira_link WHERE task_id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(task)
}

fn tags_of(conn: &Connection, task_id: i64) -> ServiceResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t.name FROM task_tag tt JOIN tag t ON t.id = tt.tag_id \
         WHERE tt.task_id = ?1 ORDER BY t.name COLLATE NOCASE",
    )?;
    let names = stmt
        .query_map([task_id], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(names)
}

/// タグ名の前後の空白と先頭の # を除き、重複（大文字小文字は区別しない）を落として名前順に並べる
fn normalize_tags(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.trim().trim_start_matches('#').trim().to_string();
        if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
            out.push(n);
        }
    }
    out.sort_by_key(|n| n.to_lowercase());
    out
}

fn set_tags(conn: &Connection, task_id: i64, names: &[String]) -> ServiceResult<()> {
    conn.execute("DELETE FROM task_tag WHERE task_id = ?1", [task_id])?;
    for name in names {
        conn.execute("INSERT OR IGNORE INTO tag (name) VALUES (?1)", [name])?;
        conn.execute(
            "INSERT OR IGNORE INTO task_tag (task_id, tag_id) \
             SELECT ?1, id FROM tag WHERE name = ?2",
            params![task_id, name],
        )?;
    }
    // どのタスクにも付いていないタグは消す
    conn.execute(
        "DELETE FROM tag WHERE id NOT IN (SELECT tag_id FROM task_tag)",
        [],
    )?;
    Ok(())
}

/// 使われているタグの一覧（名前順）
pub fn list_tags(conn: &Connection) -> ServiceResult<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(tt.task_id) FROM tag t JOIN task_tag tt ON tt.tag_id = t.id \
         GROUP BY t.id ORDER BY t.name COLLATE NOCASE",
    )?;
    let tags = stmt
        .query_map([], |r| {
            Ok(TagCount {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tags)
}

/// すべてのタスクを親ごと・並び順に返す。木の組み立ては呼び出し側で行う。
pub fn list_tasks(conn: &Connection) -> ServiceResult<Vec<Task>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS} FROM task ORDER BY parent_id, sort_order, id"
    ))?;
    let mut tasks = stmt
        .query_map([], row_to_task)?
        .collect::<Result<Vec<_>, _>>()?;
    let mut tag_stmt = conn.prepare(
        "SELECT tt.task_id, t.name FROM task_tag tt JOIN tag t ON t.id = tt.tag_id \
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let mut tags: std::collections::HashMap<i64, Vec<String>> = Default::default();
    for row in tag_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, name) = row?;
        tags.entry(id).or_default().push(name);
    }
    let mut actual: std::collections::HashMap<i64, i64> = Default::default();
    let mut actual_stmt =
        conn.prepare("SELECT task_id, SUM(minutes) FROM time_entry GROUP BY task_id")?;
    for row in actual_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (id, min) = row?;
        actual.insert(id, min);
    }
    let mut links: std::collections::HashMap<i64, String> = Default::default();
    let mut link_stmt = conn.prepare("SELECT task_id, issue_key FROM jira_link")?;
    for row in link_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, key) = row?;
        links.insert(id, key);
    }
    for t in &mut tasks {
        t.tags = tags.remove(&t.id).unwrap_or_default();
        t.jira_key = links.remove(&t.id);
        t.actual_min = actual.get(&t.id).copied().unwrap_or(0);
    }
    Ok(tasks)
}

pub fn create_task(conn: &Connection, input: NewTask) -> ServiceResult<Task> {
    let title = normalize_title(&input.title)?;
    let siblings = match input.parent_id {
        Some(p) => {
            get_task(conn, p)?;
            Siblings::Child(p)
        }
        None => Siblings::Root {
            inbox: input.inbox.unwrap_or(true),
        },
    };
    let is_inbox = matches!(siblings, Siblings::Root { inbox: true });
    let next_order: i64 = match siblings {
        Siblings::Child(p) => conn.query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM task WHERE parent_id = ?1",
            [p],
            |r| r.get(0),
        )?,
        Siblings::Root { inbox } => conn.query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM task \
             WHERE parent_id IS NULL AND is_inbox = ?1",
            [inbox],
            |r| r.get(0),
        )?,
    };
    let now = now_text();
    let tx = Tx::begin(conn)?;
    tx.execute(
        "INSERT INTO task (parent_id, title, is_inbox, sort_order, note, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            input.parent_id,
            title,
            is_inbox,
            next_order,
            normalize_note(input.note),
            now
        ],
    )?;
    let id = tx.last_insert_rowid();
    record_event(&tx, id, &now, EventKind::Created, None, Some(&title))?;
    tx.commit()?;
    get_task(conn, id)
}

pub fn update_task(conn: &Connection, id: i64, patch: TaskPatch) -> ServiceResult<Task> {
    let current = get_task(conn, id)?;
    let now = now_text();

    let title = match patch.title {
        Some(t) => normalize_title(&t)?,
        None => current.title.clone(),
    };
    let note = match patch.note {
        Some(n) => normalize_note(n),
        None => current.note.clone(),
    };
    let planned_start = match patch.planned_start {
        Some(d) => normalize_date(d)?,
        None => current.planned_start.clone(),
    };
    let planned_end = match patch.planned_end {
        Some(d) => normalize_date(d)?,
        None => current.planned_end.clone(),
    };
    // YYYY-MM-DD なので文字列の比較で前後がわかる
    if let (Some(s), Some(e)) = (&planned_start, &planned_end)
        && s > e
    {
        return Err(ServiceError::Invalid(format!(
            "開始予定日（{s}）が終了予定日（{e}）より後になっています"
        )));
    }
    let next_action = match patch.next_action {
        Some(t) => normalize_text(t),
        None => current.next_action.clone(),
    };
    let next_action_date = match patch.next_action_date {
        Some(d) => normalize_date(d)?,
        None => current.next_action_date.clone(),
    };
    let status = patch.status.unwrap_or(current.status);
    // 完了日時：状態が変わらなければそのまま、完了にしたら今、完了以外にしたら消す
    let completed_at = if status == current.status {
        current.completed_at.clone()
    } else {
        match status {
            TaskStatus::Done => Some(now.clone()),
            TaskStatus::Todo | TaskStatus::Doing => None,
        }
    };
    // 着手日時は初めて作業中にしたときだけ記録し、戻しても消さない
    let started_at = match (&current.started_at, status) {
        (None, TaskStatus::Doing) if current.status != TaskStatus::Doing => Some(now.clone()),
        (s, _) => s.clone(),
    };

    let tags = patch.tags.map(normalize_tags);
    let jira_key = match patch.jira_key {
        Some(k) => normalize_issue_key(k)?,
        None => current.jira_key.clone(),
    };
    let estimate_min = match patch.estimate_min {
        Some(Some(m)) if m < 0 => {
            return Err(ServiceError::Invalid(
                "見積もりは 0 以上にしてください".into(),
            ));
        }
        Some(m) => m,
        None => current.estimate_min,
    };

    let tx = Tx::begin(conn)?;
    if let Some(tags) = &tags
        && *tags != current.tags
    {
        set_tags(&tx, id, tags)?;
        let join = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
        record_event(
            &tx,
            id,
            &now,
            EventKind::Tags,
            join(&current.tags).as_deref(),
            join(tags).as_deref(),
        )?;
    }
    tx.execute(
        "UPDATE task SET title = ?1, note = ?2, status = ?3, completed_at = ?4, started_at = ?5, \
         planned_end = ?6, next_action = ?7, next_action_date = ?8, updated_at = ?9, \
         planned_start = ?11, estimate_min = ?12 WHERE id = ?10",
        params![
            title,
            note,
            status,
            completed_at,
            started_at,
            planned_end,
            next_action,
            next_action_date,
            now,
            id,
            planned_start,
            estimate_min
        ],
    )?;

    // いつ何を決めたか・変えたかを自動で残す（メモの変更は記録しない）
    if title != current.title {
        record_event(
            &tx,
            id,
            &now,
            EventKind::Title,
            Some(&current.title),
            Some(&title),
        )?;
    }
    if status != current.status {
        record_event(
            &tx,
            id,
            &now,
            EventKind::Status,
            Some(current.status.as_str()),
            Some(status.as_str()),
        )?;
    }
    if jira_key != current.jira_key {
        match &jira_key {
            Some(k) => tx.execute(
                "INSERT INTO jira_link (task_id, issue_key) VALUES (?1, ?2) \
                 ON CONFLICT(task_id) DO UPDATE SET issue_key = excluded.issue_key",
                params![id, k],
            )?,
            None => tx.execute("DELETE FROM jira_link WHERE task_id = ?1", [id])?,
        };
        record_event(
            &tx,
            id,
            &now,
            EventKind::Jira,
            current.jira_key.as_deref(),
            jira_key.as_deref(),
        )?;
    }
    if estimate_min != current.estimate_min {
        let text = |m: Option<i64>| m.map(|m| m.to_string());
        record_event(
            &tx,
            id,
            &now,
            EventKind::Estimate,
            text(current.estimate_min).as_deref(),
            text(estimate_min).as_deref(),
        )?;
    }
    if planned_start != current.planned_start {
        record_event(
            &tx,
            id,
            &now,
            EventKind::PlannedStart,
            current.planned_start.as_deref(),
            planned_start.as_deref(),
        )?;
    }
    if planned_end != current.planned_end {
        record_event(
            &tx,
            id,
            &now,
            EventKind::PlannedEnd,
            current.planned_end.as_deref(),
            planned_end.as_deref(),
        )?;
    }
    let old_next = next_action_text(&current.next_action_date, &current.next_action);
    let new_next = next_action_text(&next_action_date, &next_action);
    if old_next != new_next {
        record_event(
            &tx,
            id,
            &now,
            EventKind::NextAction,
            old_next.as_deref(),
            new_next.as_deref(),
        )?;
    }
    tx.commit()?;
    get_task(conn, id)
}

/// 親の付け替えと並べ替え。自分自身や子孫の下には移動できない。
pub fn move_task(conn: &Connection, id: i64, target: MoveTarget) -> ServiceResult<Task> {
    let tx = Tx::begin(conn)?;
    let task = get_task(&tx, id)?;

    if let Some(p) = target.parent_id {
        get_task(&tx, p)?;
        let is_self_or_descendant: bool = tx.query_row(
            "WITH RECURSIVE ancestors(id) AS ( \
                 SELECT ?1 \
                 UNION ALL \
                 SELECT t.parent_id FROM task t JOIN ancestors a ON t.id = a.id \
                 WHERE t.parent_id IS NOT NULL \
             ) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id = ?2)",
            params![p, id],
            |r| r.get(0),
        )?;
        if is_self_or_descendant {
            return Err(ServiceError::Invalid(
                "自分自身や子孫の下には移動できません".into(),
            ));
        }
    }

    let from = Siblings::of(&task);
    let to = match target.parent_id {
        Some(p) => Siblings::Child(p),
        None => Siblings::Root {
            inbox: target.inbox,
        },
    };

    // 移動元から外して詰める
    let mut from_ids = from.ids(&tx)?;
    from_ids.retain(|&x| x != id);
    renumber(&tx, &from_ids)?;

    // 移動先に差し込む
    let mut to_ids = if to == from { from_ids } else { to.ids(&tx)? };
    let index = target.index.unwrap_or(usize::MAX).min(to_ids.len());
    to_ids.insert(index, id);

    let is_inbox = matches!(to, Siblings::Root { inbox: true });
    tx.execute(
        "UPDATE task SET parent_id = ?1, is_inbox = ?2, updated_at = ?3 WHERE id = ?4",
        params![target.parent_id, is_inbox, now_text(), id],
    )?;
    renumber(&tx, &to_ids)?;
    tx.commit()?;
    get_task(conn, id)
}

/// 選んだタスクのうち、ほかの選んだタスクの子孫でないものだけ（並び順は保つ）。
/// 親と子を一緒に選んだときは、親を動かせば子もついてくるので親だけにする
pub(crate) fn top_level_only(conn: &Connection, ids: &[i64]) -> ServiceResult<Vec<i64>> {
    let is_descendant_of_other = |id: i64| -> ServiceResult<bool> {
        let mut cur = get_task(conn, id)?.parent_id;
        while let Some(p) = cur {
            if ids.contains(&p) {
                return Ok(true);
            }
            cur = get_task(conn, p)?.parent_id;
        }
        Ok(false)
    };
    let mut targets = Vec::new();
    for &id in ids {
        if !targets.contains(&id) && !is_descendant_of_other(id)? {
            targets.push(id);
        }
    }
    Ok(targets)
}

/// タスクを子孫ごと複製して placement の場所に置く（コピー＆貼り付け）。
/// 名前・メモ・日程・次の予定・タグ・見積もりを写す。状態は未着手に戻し、
/// Jira の課題キーは写さない（工数が同じ課題に二重に送られないように）
pub fn copy_tasks(
    conn: &Connection,
    ids: &[i64],
    placement: Placement,
) -> ServiceResult<Vec<Task>> {
    let tx = Tx::begin(conn)?;
    let mut copies = Vec::new();
    for id in top_level_only(&tx, ids)? {
        copies.push(copy_subtree(&tx, id, None)?);
    }
    let moved = move_tasks(&tx, &copies, placement)?;
    tx.commit()?;
    Ok(moved)
}

/// 1 件を複製し、子を同じ順で複製してその下に付ける。複製の id を返す
fn copy_subtree(conn: &Connection, id: i64, parent_id: Option<i64>) -> ServiceResult<i64> {
    let t = get_task(conn, id)?;
    let copy = create_task(
        conn,
        NewTask {
            title: t.title.clone(),
            parent_id,
            note: t.note.clone(),
            inbox: Some(true),
        },
    )?;
    update_task(
        conn,
        copy.id,
        TaskPatch {
            planned_start: Some(t.planned_start.clone()),
            planned_end: Some(t.planned_end.clone()),
            next_action: Some(t.next_action.clone()),
            next_action_date: Some(t.next_action_date.clone()),
            tags: Some(t.tags.clone()),
            estimate_min: Some(t.estimate_min),
            ..Default::default()
        },
    )?;
    for child in Siblings::Child(id).ids(conn)? {
        copy_subtree(conn, child, Some(copy.id))?;
    }
    Ok(copy.id)
}

/// 複数行の文字から、1 行 1 タスクで作って placement の場所に置く（ほかのアプリからの貼り付け）
pub fn create_tasks(
    conn: &Connection,
    titles: &[String],
    placement: Placement,
) -> ServiceResult<Vec<Task>> {
    let tx = Tx::begin(conn)?;
    let mut ids = Vec::new();
    for title in titles.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        ids.push(
            create_task(
                &tx,
                NewTask {
                    title: title.to_string(),
                    inbox: Some(true),
                    ..Default::default()
                },
            )?
            .id,
        );
    }
    if ids.is_empty() {
        return Err(ServiceError::Invalid("貼り付ける文字がありません".into()));
    }
    let moved = move_tasks(&tx, &ids, placement)?;
    tx.commit()?;
    Ok(moved)
}

/// 複数のタスクを、並び順を保ったまままとめて移動する。
/// ids は画面での並び順。ほかの選択されたタスクの子孫は、親と一緒に動くので個別には動かさない
pub fn move_tasks(
    conn: &Connection,
    ids: &[i64],
    placement: Placement,
) -> ServiceResult<Vec<Task>> {
    let tx = Tx::begin(conn)?;
    let targets = top_level_only(&tx, ids)?;
    // 基準のタスクを動かす対象にはできない
    if let Placement::Before { ref_id } | Placement::After { ref_id } = placement
        && targets.contains(&ref_id)
    {
        return Err(ServiceError::Invalid(
            "選んだタスクの前後には移動できません".into(),
        ));
    }

    // 基準のタスクの兄弟の中での位置（動かすタスク自身は除いて数える）
    let place_near = |id: i64, ref_id: i64, after: bool| -> ServiceResult<Task> {
        let r = get_task(&tx, ref_id)?;
        let siblings: Vec<i64> = Siblings::of(&r)
            .ids(&tx)?
            .into_iter()
            .filter(|&x| x != id)
            .collect();
        let pos = siblings
            .iter()
            .position(|&x| x == ref_id)
            .unwrap_or(siblings.len());
        move_task(
            &tx,
            id,
            MoveTarget {
                parent_id: r.parent_id,
                inbox: r.parent_id.is_none() && r.is_inbox,
                index: Some(if after { pos + 1 } else { pos }),
            },
        )
    };
    let mut moved = Vec::new();
    match placement {
        Placement::Before { ref_id } => {
            for &id in &targets {
                moved.push(place_near(id, ref_id, false)?);
            }
        }
        Placement::After { ref_id } => {
            // 1 件目を基準の後ろに、2 件目以降は直前に動かしたものの後ろに
            let mut prev = ref_id;
            for &id in &targets {
                moved.push(place_near(id, prev, true)?);
                prev = id;
            }
        }
        Placement::Into { parent_id, inbox } => {
            for &id in &targets {
                moved.push(move_task(
                    &tx,
                    id,
                    MoveTarget {
                        parent_id,
                        inbox,
                        index: None,
                    },
                )?);
            }
        }
    }
    tx.commit()?;
    Ok(moved)
}

/// 複数のタスクを子孫ごとまとめて削除する（ほかの選んだタスクの子孫は親と一緒に消えるので個別には消さない）
pub fn delete_tasks(conn: &Connection, ids: &[i64]) -> ServiceResult<usize> {
    let tx = Tx::begin(conn)?;
    let targets = top_level_only(&tx, ids)?;
    for &id in &targets {
        delete_task(&tx, id)?;
    }
    tx.commit()?;
    Ok(targets.len())
}

/// 子孫もまとめて削除する。
pub fn delete_task(conn: &Connection, id: i64) -> ServiceResult<()> {
    let tx = Tx::begin(conn)?;
    let task = get_task(&tx, id)?;
    tx.execute("DELETE FROM task WHERE id = ?1", [id])?;
    let ids = Siblings::of(&task).ids(&tx)?;
    renumber(&tx, &ids)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn add(conn: &Connection, title: &str, parent_id: Option<i64>) -> Task {
        create_task(
            conn,
            NewTask {
                title: title.into(),
                parent_id,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn child_titles(conn: &Connection, s: Siblings) -> Vec<String> {
        s.ids(conn)
            .unwrap()
            .into_iter()
            .map(|id| get_task(conn, id).unwrap().title)
            .collect()
    }

    #[test]
    fn title_only_goes_to_inbox() {
        let conn = db::open_in_memory().unwrap();
        let t = add(&conn, "  メモ  ", None);
        assert_eq!(t.title, "メモ");
        assert!(t.is_inbox);
        assert_eq!(t.status, TaskStatus::Todo);
    }

    #[test]
    fn empty_title_is_rejected() {
        let conn = db::open_in_memory().unwrap();
        let err = create_task(&conn, NewTask::default()).unwrap_err();
        assert!(matches!(err, ServiceError::Invalid(_)));
    }

    #[test]
    fn children_are_appended_in_order() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None);
        add(&conn, "a", Some(p.id));
        let b = add(&conn, "b", Some(p.id));
        assert!(!b.is_inbox);
        assert_eq!(b.sort_order, 1);
        assert_eq!(child_titles(&conn, Siblings::Child(p.id)), ["a", "b"]);
    }

    #[test]
    fn reorder_within_siblings() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None);
        add(&conn, "a", Some(p.id));
        add(&conn, "b", Some(p.id));
        let c = add(&conn, "c", Some(p.id));
        move_task(
            &conn,
            c.id,
            MoveTarget {
                parent_id: Some(p.id),
                index: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(child_titles(&conn, Siblings::Child(p.id)), ["c", "a", "b"]);
    }

    #[test]
    fn move_out_of_inbox_and_under_parent() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let b = add(&conn, "b", None);
        let a = move_task(&conn, a.id, MoveTarget::default()).unwrap();
        assert!(!a.is_inbox);
        assert_eq!(child_titles(&conn, Siblings::Root { inbox: true }), ["b"]);

        let b = move_task(
            &conn,
            b.id,
            MoveTarget {
                parent_id: Some(a.id),
                inbox: true,
                index: None,
            },
        )
        .unwrap();
        assert_eq!(b.parent_id, Some(a.id));
        assert!(!b.is_inbox);
        assert!(child_titles(&conn, Siblings::Root { inbox: true }).is_empty());
    }

    #[test]
    fn cannot_move_under_descendant() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let b = add(&conn, "b", Some(a.id));
        let c = add(&conn, "c", Some(b.id));
        for target in [a.id, c.id] {
            let err = move_task(
                &conn,
                a.id,
                MoveTarget {
                    parent_id: Some(target),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert!(matches!(err, ServiceError::Invalid(_)));
        }
    }

    #[test]
    fn done_sets_completed_at_and_reopen_clears_it() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let done = update_task(
            &conn,
            a.id,
            TaskPatch {
                status: Some(TaskStatus::Done),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(done.completed_at.is_some());
        let reopened = update_task(
            &conn,
            a.id,
            TaskPatch {
                status: Some(TaskStatus::Todo),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(reopened.completed_at.is_none());
    }

    #[test]
    fn schedule_fields_and_history_are_recorded() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let patch: TaskPatch = serde_json::from_str(
            r#"{"planned_end":"2026-11-30","next_action_date":"2026-11-25","next_action":" 課長にレビュー依頼 "}"#,
        )
        .unwrap();
        let a = update_task(&conn, a.id, patch).unwrap();
        assert_eq!(a.planned_end.as_deref(), Some("2026-11-30"));
        assert_eq!(a.next_action.as_deref(), Some("課長にレビュー依頼"));
        assert_eq!(a.next_action_date.as_deref(), Some("2026-11-25"));

        // 予定日をずらす → 履歴に旧値と新値が残る
        let patch: TaskPatch = serde_json::from_str(r#"{"planned_end":"2026-12-04"}"#).unwrap();
        update_task(&conn, a.id, patch).unwrap();
        // null で消す
        let patch: TaskPatch =
            serde_json::from_str(r#"{"next_action":null,"next_action_date":null}"#).unwrap();
        let a = update_task(&conn, a.id, patch).unwrap();
        assert!(a.next_action.is_none() && a.next_action_date.is_none());
        assert_eq!(a.planned_end.as_deref(), Some("2026-12-04"));

        let events: Vec<_> = list_events(&conn, a.id)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind.as_str().to_string(), e.old_value, e.new_value))
            .collect();
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            events,
            [
                ("created".into(), None, s("a")),
                ("planned_end".into(), None, s("2026-11-30")),
                (
                    "next_action".into(),
                    None,
                    s("2026-11-25 課長にレビュー依頼")
                ),
                ("planned_end".into(), s("2026-11-30"), s("2026-12-04")),
                (
                    "next_action".into(),
                    s("2026-11-25 課長にレビュー依頼"),
                    None
                ),
            ]
        );
    }

    #[test]
    fn rapid_edits_are_merged_into_one_event() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let set_end = |d: &str| {
            let patch: TaskPatch =
                serde_json::from_str(&format!(r#"{{"planned_end":"{d}"}}"#)).unwrap();
            update_task(&conn, a.id, patch).unwrap();
        };
        set_end("2026-11-01");
        set_end("2026-11-02");
        set_end("2026-11-30");
        let events = list_events(&conn, a.id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].old_value, None);
        assert_eq!(events[1].new_value.as_deref(), Some("2026-11-30"));

        // 元に戻したら記録ごと消える
        let patch: TaskPatch = serde_json::from_str(r#"{"planned_end":null}"#).unwrap();
        update_task(&conn, a.id, patch).unwrap();
        assert_eq!(list_events(&conn, a.id).unwrap().len(), 1);
    }

    #[test]
    fn planned_start_must_not_be_after_end() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let patch: TaskPatch =
            serde_json::from_str(r#"{"planned_start":"2026-11-23","planned_end":"2026-11-27"}"#)
                .unwrap();
        let a = update_task(&conn, a.id, patch).unwrap();
        assert_eq!(a.planned_start.as_deref(), Some("2026-11-23"));
        let patch: TaskPatch = serde_json::from_str(r#"{"planned_end":"2026-11-20"}"#).unwrap();
        assert!(matches!(
            update_task(&conn, a.id, patch),
            Err(ServiceError::Invalid(_))
        ));
    }

    #[test]
    fn tags_are_normalized_and_cleaned_up() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let b = add(&conn, "b", None);
        let set = |id, tags: &[&str]| {
            update_task(
                &conn,
                id,
                TaskPatch {
                    tags: Some(tags.iter().map(|s| s.to_string()).collect()),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let a = set(a.id, &[" #案件A ", "Work", "work", "", "会議"]);
        assert_eq!(a.tags, ["Work", "会議", "案件A"]);
        set(b.id, &["work"]);
        let tags = list_tags(&conn).unwrap();
        let work = tags.iter().find(|t| t.name == "Work").unwrap();
        assert_eq!(work.count, 2);

        set(a.id, &["Work"]);
        // 使われなくなったタグは消える
        let names: Vec<_> = list_tags(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["Work"]);
        let all = list_tasks(&conn).unwrap();
        assert_eq!(all.iter().find(|t| t.id == b.id).unwrap().tags, ["Work"]);

        let ev = list_events(&conn, a.id).unwrap();
        let last = ev.last().unwrap();
        assert_eq!(last.kind, EventKind::Tags);
        assert_eq!(last.new_value.as_deref(), Some("Work"));
    }

    #[test]
    fn jira_key_is_normalized_and_validated() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let set = |k: &str| {
            let p: TaskPatch = serde_json::from_str(&format!(r#"{{"jira_key":{k}}}"#)).unwrap();
            update_task(&conn, a.id, p)
        };
        assert_eq!(
            set(r#"" abc-12 ""#).unwrap().jira_key.as_deref(),
            Some("ABC-12")
        );
        assert_eq!(
            list_tasks(&conn).unwrap()[0].jira_key.as_deref(),
            Some("ABC-12")
        );
        assert!(matches!(set(r#""12-ABC""#), Err(ServiceError::Invalid(_))));
        assert!(matches!(set(r#""ABC""#), Err(ServiceError::Invalid(_))));
        assert_eq!(set("null").unwrap().jira_key, None);
    }

    #[test]
    fn move_many_keeps_order_and_skips_children_of_selected() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None).id;
        let a = add(&conn, "a", Some(p)).id;
        let b = add(&conn, "b", Some(p)).id;
        let c = add(&conn, "c", Some(p)).id;
        let x = add(&conn, "x", None).id;
        let x1 = add(&conn, "x1", Some(x)).id;
        let titles = |s: Siblings| child_titles(&conn, s);

        // b, x（とその子 x1）を c の後ろへ。x1 は x と一緒に動く
        move_tasks(&conn, &[b, x, x1], Placement::After { ref_id: c }).unwrap();
        assert_eq!(titles(Siblings::Child(p)), ["a", "c", "b", "x"]);
        assert_eq!(get_task(&conn, x1).unwrap().parent_id, Some(x));

        // c, x を a の前へ
        move_tasks(&conn, &[c, x], Placement::Before { ref_id: a }).unwrap();
        assert_eq!(titles(Siblings::Child(p)), ["c", "x", "a", "b"]);

        // a, b を未分類の末尾へ
        move_tasks(
            &conn,
            &[a, b],
            Placement::Into {
                parent_id: None,
                inbox: true,
            },
        )
        .unwrap();
        assert_eq!(titles(Siblings::Child(p)), ["c", "x"]);
        assert_eq!(titles(Siblings::Root { inbox: true }), ["親", "a", "b"]);

        // 自分の子孫の下へは移せず、途中まで動いたものも戻る
        let err = move_tasks(
            &conn,
            &[a, p],
            Placement::Into {
                parent_id: Some(x1),
                inbox: false,
            },
        );
        assert!(err.is_err());
        assert_eq!(titles(Siblings::Root { inbox: true }), ["親", "a", "b"]);
    }

    #[test]
    fn copy_duplicates_subtree_with_fields_but_fresh_state() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None).id;
        let a = add(&conn, "a", Some(p)).id;
        let a1 = add(&conn, "a1", Some(a)).id;
        add(&conn, "a2", Some(a));
        let b = add(&conn, "b", Some(p)).id;
        let patch: TaskPatch = serde_json::from_str(
            r#"{"status":"done","planned_start":"2026-11-20","planned_end":"2026-11-25","tags":["x"],"estimate_min":120,"jira_key":"ABC-1","note":"メモ"}"#,
        )
        .unwrap();
        update_task(&conn, a, patch).unwrap();

        // a と、その子の a1 を一緒にコピー → a1 は a と一緒に複製されるので 1 回だけ
        let copies = copy_tasks(&conn, &[a, a1], Placement::After { ref_id: b }).unwrap();
        assert_eq!(copies.len(), 1);
        let c = &copies[0];
        assert_eq!(c.parent_id, Some(p));
        assert_eq!((c.title.as_str(), c.note.as_deref()), ("a", Some("メモ")));
        assert_eq!(
            (c.planned_start.as_deref(), c.planned_end.as_deref()),
            (Some("2026-11-20"), Some("2026-11-25"))
        );
        assert_eq!(
            (c.tags.clone(), c.estimate_min),
            (vec!["x".to_string()], Some(120))
        );
        assert_eq!(c.status, TaskStatus::Todo);
        assert_eq!(c.jira_key, None);
        assert_eq!(child_titles(&conn, Siblings::Child(p)), ["a", "b", "a"]);
        assert_eq!(child_titles(&conn, Siblings::Child(c.id)), ["a1", "a2"]);
    }

    #[test]
    fn create_tasks_from_lines() {
        let conn = db::open_in_memory().unwrap();
        let made = create_tasks(
            &conn,
            &["一つ目".into(), "  ".into(), " 二つ目 ".into()],
            Placement::Into {
                parent_id: None,
                inbox: true,
            },
        )
        .unwrap();
        let titles: Vec<_> = made.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["一つ目", "二つ目"]);
        assert!(
            create_tasks(
                &conn,
                &["".into()],
                Placement::Into {
                    parent_id: None,
                    inbox: true
                }
            )
            .is_err()
        );
    }

    #[test]
    fn delete_many_removes_subtrees_once() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None).id;
        let a = add(&conn, "a", Some(p)).id;
        let a1 = add(&conn, "a1", Some(a)).id;
        let b = add(&conn, "b", Some(p)).id;
        add(&conn, "c", Some(p));
        // a とその子 a1、b を選んで削除 → a1 は a と一緒に消える
        assert_eq!(delete_tasks(&conn, &[a, a1, b]).unwrap(), 2);
        assert_eq!(child_titles(&conn, Siblings::Child(p)), ["c"]);
        assert!(get_task(&conn, a1).is_err());
        // 存在しないタスクが混じっていたら何も消さない
        assert!(delete_tasks(&conn, &[p, 9999]).is_err());
        assert!(get_task(&conn, p).is_ok());
    }

    #[test]
    fn invalid_date_is_rejected() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        let patch: TaskPatch = serde_json::from_str(r#"{"planned_end":"11/30"}"#).unwrap();
        assert!(matches!(
            update_task(&conn, a.id, patch),
            Err(ServiceError::Invalid(_))
        ));
    }

    #[test]
    fn started_at_is_set_once() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "a", None);
        assert!(a.started_at.is_none());
        let set = |s| TaskPatch {
            status: Some(s),
            ..Default::default()
        };
        let doing = update_task(&conn, a.id, set(TaskStatus::Doing)).unwrap();
        assert!(doing.started_at.is_some());
        let back = update_task(&conn, a.id, set(TaskStatus::Todo)).unwrap();
        assert_eq!(back.started_at, doing.started_at);

        // 状態を変えない編集では記録しない（着手日時のない作業中タスク）
        conn.execute(
            "UPDATE task SET started_at = NULL, status = ?2 WHERE id = ?1",
            rusqlite::params![a.id, TaskStatus::Doing],
        )
        .unwrap();
        let edited = update_task(
            &conn,
            a.id,
            TaskPatch {
                title: Some("b".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(edited.started_at.is_none());
    }

    #[test]
    fn delete_cascades_and_renumbers() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "親", None);
        let a = add(&conn, "a", Some(p.id));
        add(&conn, "a1", Some(a.id));
        add(&conn, "b", Some(p.id));
        delete_task(&conn, a.id).unwrap();
        assert_eq!(list_tasks(&conn).unwrap().len(), 2);
        let b = get_task(&conn, Siblings::Child(p.id).ids(&conn).unwrap()[0]).unwrap();
        assert_eq!((b.title.as_str(), b.sort_order), ("b", 0));
    }
}
