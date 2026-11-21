//! テンプレート：既存のタスク（子を含む）を型として保存し、別の機会に開始日を選んで作り直す。
//! 日付は「基準日（選んだ木の中で一番早い日付）から何日目か」で保存するので、
//! 使うときに開始日を選ぶと、同じ間隔のままずれた日程で作られる。

use jiff::ToSpan;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::calendar;
use super::task_service::{self, Siblings};
use super::{ServiceError, ServiceResult, Tx, now_text, parse_date};
use crate::model::{NewTask, Placement, Task, TaskPatch};

str_enum! {
    /// テンプレートの日付（offset）の数え方
    pub enum OffsetUnit {
        /// 暦日（前からあるテンプレート）
        Calendar => "calendar",
        /// 平日（土日を除く）。使うときも平日で数えるので、土日をまたぐと伸びる
        Workday => "workday",
    }
}

impl OffsetUnit {
    /// base から d まで何日目か
    fn offset(self, base: Date, d: Date) -> ServiceResult<i64> {
        match self {
            OffsetUnit::Calendar => {
                let span = base
                    .until(d)
                    .map_err(|e| ServiceError::Invalid(format!("日付の差を計算できません: {e}")))?;
                Ok(i64::from(span.get_days()))
            }
            OffsetUnit::Workday => calendar::weekdays_between(base, d),
        }
    }

    /// start から offset 日目の日
    fn shift(self, start: Date, offset: i64) -> ServiceResult<Date> {
        match self {
            OffsetUnit::Calendar => start
                .checked_add(offset.days())
                .map_err(|e| ServiceError::Invalid(format!("日付を計算できません: {e}"))),
            OffsetUnit::Workday => calendar::add_weekdays(start, offset),
        }
    }
}

/// 一覧に出す 1 件
#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    pub id: i64,
    pub name: String,
    /// 項目（タスク）の数（子を含む）
    pub item_count: i64,
    /// 一番早い日付から一番遅い日付までの日数（両端を含む。offset_unit で数える）。日付がなければ None
    pub span_days: Option<i64>,
    pub offset_unit: OffsetUnit,
    pub created_at: String,
}

/// テンプレートの 1 項目（子を含む木）
#[derive(Debug, Serialize)]
pub struct TemplateItem {
    pub title: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub estimate_min: Option<i64>,
    /// 開始予定日（基準日から何日目か。0 が基準日）
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
    pub next_action: Option<String>,
    pub next_action_offset: Option<i64>,
    pub children: Vec<TemplateItem>,
}

#[derive(Debug, Serialize)]
pub struct Template {
    #[serde(flatten)]
    pub summary: TemplateSummary,
    pub items: Vec<TemplateItem>,
}

fn normalize_name(name: &str) -> ServiceResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ServiceError::Invalid(
            "テンプレートの名前を入れてください".into(),
        ));
    }
    Ok(name.to_string())
}

/// 木の中のタスクを、親→子の順に（兄弟は並び順で）集める
fn collect_tree(conn: &Connection, id: i64, out: &mut Vec<Task>) -> ServiceResult<()> {
    out.push(task_service::get_task(conn, id)?);
    for child in Siblings::Child(id).ids(conn)? {
        collect_tree(conn, child, out)?;
    }
    Ok(())
}

/// 新しく保存するテンプレートの数え方
const SAVE_UNIT: OffsetUnit = OffsetUnit::Workday;

fn offset_of(base: Option<Date>, date: &Option<String>) -> ServiceResult<Option<i64>> {
    match (base, date) {
        (Some(base), Some(d)) => Ok(Some(SAVE_UNIT.offset(base, parse_date(d)?)?)),
        (None, _) | (_, None) => Ok(None),
    }
}

fn insert_item(
    conn: &Connection,
    template_id: i64,
    parent_item_id: Option<i64>,
    sort_order: i64,
    task: &Task,
    base: Option<Date>,
) -> ServiceResult<i64> {
    let tags = serde_json::to_string(&task.tags)
        .map_err(|e| ServiceError::Invalid(format!("タグを保存できません: {e}")))?;
    conn.execute(
        "INSERT INTO template_item (template_id, parent_item_id, sort_order, title, note, tags, \
         estimate_min, start_offset, end_offset, next_action, next_action_offset) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            template_id,
            parent_item_id,
            sort_order,
            task.title,
            task.note,
            tags,
            task.estimate_min,
            offset_of(base, &task.planned_start)?,
            offset_of(base, &task.planned_end)?,
            task.next_action,
            offset_of(base, &task.next_action_date)?,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn save_subtree(
    conn: &Connection,
    template_id: i64,
    parent_item_id: Option<i64>,
    sort_order: i64,
    id: i64,
    base: Option<Date>,
) -> ServiceResult<()> {
    let task = task_service::get_task(conn, id)?;
    let item_id = insert_item(conn, template_id, parent_item_id, sort_order, &task, base)?;
    for (i, child) in Siblings::Child(id).ids(conn)?.into_iter().enumerate() {
        save_subtree(conn, template_id, Some(item_id), i as i64, child, base)?;
    }
    Ok(())
}

/// 選んだタスク（子を含む）をテンプレートとして保存する。ids は画面での並び順
pub fn save_template(conn: &Connection, name: &str, ids: &[i64]) -> ServiceResult<TemplateSummary> {
    let name = normalize_name(name)?;
    let tx = Tx::begin(conn)?;
    let roots = task_service::top_level_only(&tx, ids)?;
    if roots.is_empty() {
        return Err(ServiceError::Invalid(
            "テンプレートにするタスクを選んでください".into(),
        ));
    }
    // 基準日：選んだ木の中で一番早い日付
    let mut all = Vec::new();
    for &id in &roots {
        collect_tree(&tx, id, &mut all)?;
    }
    let mut base: Option<Date> = None;
    for t in &all {
        for d in [&t.planned_start, &t.planned_end, &t.next_action_date]
            .into_iter()
            .flatten()
        {
            let d = parse_date(d)?;
            base = Some(base.map_or(d, |b| b.min(d)));
        }
    }
    tx.execute(
        "INSERT INTO template (name, created_at, offset_unit) VALUES (?1, ?2, ?3)",
        params![name, now_text(), SAVE_UNIT],
    )?;
    let template_id = tx.last_insert_rowid();
    for (i, &id) in roots.iter().enumerate() {
        save_subtree(&tx, template_id, None, i as i64, id, base)?;
    }
    let summary = get_summary(&tx, template_id)?;
    tx.commit()?;
    Ok(summary)
}

const SUMMARY_SQL: &str = "SELECT t.id, t.name, t.created_at, t.offset_unit, \
     (SELECT COUNT(*) FROM template_item i WHERE i.template_id = t.id), \
     (SELECT MAX(MAX(COALESCE(start_offset, -1e9), COALESCE(end_offset, -1e9), COALESCE(next_action_offset, -1e9))) \
        FROM template_item i WHERE i.template_id = t.id), \
     (SELECT MIN(MIN(COALESCE(start_offset, 1e9), COALESCE(end_offset, 1e9), COALESCE(next_action_offset, 1e9))) \
        FROM template_item i WHERE i.template_id = t.id) \
     FROM template t";

fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<TemplateSummary> {
    let max: Option<f64> = r.get(5)?;
    let min: Option<f64> = r.get(6)?;
    // 日付が 1 つもなければ番兵の値のまま
    let span_days = match (max, min) {
        (Some(max), Some(min)) if max > -1e8 && min < 1e8 => Some((max - min) as i64 + 1),
        _ => None,
    };
    Ok(TemplateSummary {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        offset_unit: r.get(3)?,
        item_count: r.get(4)?,
        span_days,
    })
}

fn get_summary(conn: &Connection, id: i64) -> ServiceResult<TemplateSummary> {
    conn.query_row(
        &format!("{SUMMARY_SQL} WHERE t.id = ?1"),
        [id],
        row_to_summary,
    )
    .optional()?
    .ok_or_else(|| ServiceError::Invalid(format!("テンプレート {id} がありません")))
}

/// テンプレートの一覧（名前順）
pub fn list_templates(conn: &Connection) -> ServiceResult<Vec<TemplateSummary>> {
    let mut stmt = conn.prepare(&format!(
        "{SUMMARY_SQL} ORDER BY t.name COLLATE NOCASE, t.id"
    ))?;
    let list = stmt
        .query_map([], row_to_summary)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(list)
}

fn load_items(
    conn: &Connection,
    template_id: i64,
    parent: Option<i64>,
) -> ServiceResult<Vec<TemplateItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, note, tags, estimate_min, start_offset, end_offset, next_action, next_action_offset \
         FROM template_item WHERE template_id = ?1 AND parent_item_id IS ?2 ORDER BY sort_order, id",
    )?;
    let rows = stmt
        .query_map(params![template_id, parent], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<i64>>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut items = Vec::new();
    for (
        id,
        title,
        note,
        tags,
        estimate_min,
        start_offset,
        end_offset,
        next_action,
        next_action_offset,
    ) in rows
    {
        items.push(TemplateItem {
            title,
            note,
            tags: serde_json::from_str(&tags).unwrap_or_default(),
            estimate_min,
            start_offset,
            end_offset,
            next_action,
            next_action_offset,
            children: load_items(conn, template_id, Some(id))?,
        });
    }
    Ok(items)
}

/// テンプレートの中身（項目の木）
pub fn get_template(conn: &Connection, id: i64) -> ServiceResult<Template> {
    Ok(Template {
        summary: get_summary(conn, id)?,
        items: load_items(conn, id, None)?,
    })
}

pub fn rename_template(conn: &Connection, id: i64, name: &str) -> ServiceResult<TemplateSummary> {
    let name = normalize_name(name)?;
    let tx = Tx::begin(conn)?;
    get_summary(&tx, id)?;
    tx.execute(
        "UPDATE template SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    let summary = get_summary(&tx, id)?;
    tx.commit()?;
    Ok(summary)
}

pub fn delete_template(conn: &Connection, id: i64) -> ServiceResult<()> {
    let tx = Tx::begin(conn)?;
    get_summary(&tx, id)?;
    tx.execute("DELETE FROM template WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

fn shifted(unit: OffsetUnit, start: Date, offset: Option<i64>) -> ServiceResult<Option<String>> {
    offset
        .map(|o| unit.shift(start, o).map(|d| d.to_string()))
        .transpose()
}

fn create_from_item(
    conn: &Connection,
    item: &TemplateItem,
    parent_id: Option<i64>,
    unit: OffsetUnit,
    start: Date,
    created: &mut usize,
) -> ServiceResult<i64> {
    let task = task_service::create_task(
        conn,
        NewTask {
            title: item.title.clone(),
            parent_id,
            note: item.note.clone(),
            inbox: Some(true),
        },
    )?;
    task_service::update_task(
        conn,
        task.id,
        TaskPatch {
            planned_start: Some(shifted(unit, start, item.start_offset)?),
            planned_end: Some(shifted(unit, start, item.end_offset)?),
            next_action: Some(item.next_action.clone()),
            next_action_date: Some(shifted(unit, start, item.next_action_offset)?),
            tags: Some(item.tags.clone()),
            estimate_min: Some(item.estimate_min),
            ..Default::default()
        },
    )?;
    *created += 1;
    for child in &item.children {
        create_from_item(conn, child, Some(task.id), unit, start, created)?;
    }
    Ok(task.id)
}

/// テンプレートからタスクを作って placement の場所に置く。
/// 日付は start_date を基準日として、保存したときと同じ間隔（平日のテンプレートは平日の数）でずらす。
/// 作った件数（子を含む）を返す
pub fn apply_template(
    conn: &Connection,
    id: i64,
    start_date: &str,
    placement: Placement,
) -> ServiceResult<usize> {
    let start = parse_date(start_date)?;
    let tx = Tx::begin(conn)?;
    let template = get_template(&tx, id)?;
    let mut created = 0;
    let mut roots = Vec::new();
    for item in &template.items {
        roots.push(create_from_item(
            &tx,
            item,
            None,
            template.summary.offset_unit,
            start,
            &mut created,
        )?);
    }
    if roots.is_empty() {
        return Err(ServiceError::Invalid("テンプレートが空です".into()));
    }
    task_service::move_tasks(&tx, &roots, placement)?;
    tx.commit()?;
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::service::undo_service;

    fn add(
        conn: &Connection,
        title: &str,
        parent_id: Option<i64>,
        start: Option<&str>,
        end: Option<&str>,
    ) -> i64 {
        let t = task_service::create_task(
            conn,
            NewTask {
                title: title.into(),
                parent_id,
                inbox: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        task_service::update_task(
            conn,
            t.id,
            TaskPatch {
                planned_start: Some(start.map(String::from)),
                planned_end: Some(end.map(String::from)),
                ..Default::default()
            },
        )
        .unwrap();
        t.id
    }

    #[test]
    fn save_and_apply_shifts_dates() {
        let conn = db::open_in_memory().unwrap();
        let p = add(
            &conn,
            "リリース",
            None,
            Some("2026-03-02"),
            Some("2026-03-10"),
        );
        let c1 = add(
            &conn,
            "準備",
            Some(p),
            Some("2026-03-02"),
            Some("2026-03-03"),
        );
        add(&conn, "本番", Some(p), None, Some("2026-03-10"));
        task_service::update_task(
            &conn,
            c1,
            TaskPatch {
                tags: Some(vec!["作業".into()]),
                estimate_min: Some(Some(240)),
                status: Some(crate::model::TaskStatus::Done),
                ..Default::default()
            },
        )
        .unwrap();

        // 子だけ一緒に選んでも親の木 1 つになる
        let s = save_template(&conn, " 月次リリース ", &[p, c1]).unwrap();
        assert_eq!(s.name, "月次リリース");
        assert_eq!(s.item_count, 3);
        // 3/2（月）〜3/10（火）は平日で 7 日
        assert_eq!(s.span_days, Some(7));
        assert_eq!(s.offset_unit, OffsetUnit::Workday);
        let tpl = get_template(&conn, s.id).unwrap();
        assert_eq!(tpl.items.len(), 1);
        assert_eq!(tpl.items[0].children[0].end_offset, Some(1));
        assert_eq!(tpl.items[0].children[1].start_offset, None);

        let n = apply_template(
            &conn,
            s.id,
            "2026-04-06",
            Placement::Into {
                parent_id: None,
                inbox: false,
            },
        )
        .unwrap();
        assert_eq!(n, 3);
        let tasks = task_service::list_tasks(&conn).unwrap();
        let new_root = tasks
            .iter()
            .filter(|t| t.title == "リリース" && t.parent_id.is_none())
            .max_by_key(|t| t.id)
            .unwrap();
        assert_eq!(new_root.planned_start.as_deref(), Some("2026-04-06"));
        assert_eq!(new_root.planned_end.as_deref(), Some("2026-04-14"));
        assert!(!new_root.is_inbox);
        let kids: Vec<&Task> = {
            let mut k: Vec<&Task> = tasks
                .iter()
                .filter(|t| t.parent_id == Some(new_root.id))
                .collect();
            k.sort_by_key(|t| t.sort_order);
            k
        };
        assert_eq!(
            kids.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["準備", "本番"]
        );
        assert_eq!(kids[0].planned_end.as_deref(), Some("2026-04-07"));
        assert_eq!(kids[0].tags, ["作業"]);
        assert_eq!(kids[0].estimate_min, Some(240));
        assert_eq!(kids[0].status, crate::model::TaskStatus::Todo);
        assert_eq!(kids[1].planned_start, None);
        assert_eq!(kids[1].planned_end.as_deref(), Some("2026-04-14"));
    }

    /// 平日で数えるので、使う先の土日を飛ばして伸びる
    #[test]
    fn apply_skips_weekends() {
        let conn = db::open_in_memory().unwrap();
        // 保存元：11/18（水）〜11/20（金）の 3 平日、子は 11/20 だけ
        let p = add(&conn, "作業", None, Some("2026-11-18"), Some("2026-11-20"));
        add(&conn, "締め", Some(p), None, Some("2026-11-20"));
        let s = save_template(&conn, "3 日", &[p]).unwrap();
        assert_eq!(s.span_days, Some(3));
        // 4/28（火）から：土日をまたがないので 4/28〜4/30（祝日は考えない）
        apply_template(
            &conn,
            s.id,
            "2026-04-28",
            Placement::Into {
                parent_id: None,
                inbox: false,
            },
        )
        .unwrap();
        let tasks = task_service::list_tasks(&conn).unwrap();
        let root = tasks
            .iter()
            .filter(|t| t.title == "作業")
            .max_by_key(|t| t.id)
            .unwrap();
        assert_eq!(root.planned_start.as_deref(), Some("2026-04-28"));
        assert_eq!(root.planned_end.as_deref(), Some("2026-04-30"));
        // 5/1（金）から：土日を飛ばして 5/1・5/4・5/5
        apply_template(
            &conn,
            s.id,
            "2026-05-01",
            Placement::Into {
                parent_id: None,
                inbox: false,
            },
        )
        .unwrap();
        let tasks = task_service::list_tasks(&conn).unwrap();
        let root = tasks
            .iter()
            .filter(|t| t.title == "作業")
            .max_by_key(|t| t.id)
            .unwrap();
        assert_eq!(root.planned_end.as_deref(), Some("2026-05-05"));
        let child = tasks.iter().find(|t| t.parent_id == Some(root.id)).unwrap();
        assert_eq!(child.planned_end.as_deref(), Some("2026-05-05"));
    }

    /// 前からある暦日のテンプレートは、暦日のままずらす
    #[test]
    fn calendar_templates_keep_calendar_days() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "暦日", None, Some("2026-11-20"), Some("2026-11-23"));
        let s = save_template(&conn, "暦日", &[p]).unwrap();
        conn.execute("UPDATE template SET offset_unit = 'calendar'", [])
            .unwrap();
        conn.execute("UPDATE template_item SET end_offset = 3", [])
            .unwrap();
        assert_eq!(list_templates(&conn).unwrap()[0].span_days, Some(4));
        apply_template(
            &conn,
            s.id,
            "2026-04-28",
            Placement::Into {
                parent_id: None,
                inbox: false,
            },
        )
        .unwrap();
        let tasks = task_service::list_tasks(&conn).unwrap();
        let root = tasks
            .iter()
            .filter(|t| t.title == "暦日")
            .max_by_key(|t| t.id)
            .unwrap();
        assert_eq!(root.planned_end.as_deref(), Some("2026-05-01"));
    }

    #[test]
    fn template_without_dates() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "A", None, None, None);
        let b = add(&conn, "B", None, None, None);
        let s = save_template(&conn, "日付なし", &[b, a]).unwrap();
        assert_eq!(s.span_days, None);
        assert_eq!(s.item_count, 2);
        let n = apply_template(&conn, s.id, "2026-01-01", Placement::After { ref_id: a }).unwrap();
        assert_eq!(n, 2);
        let roots: Vec<String> = Siblings::Root { inbox: false }
            .ids(&conn)
            .unwrap()
            .into_iter()
            .map(|id| task_service::get_task(&conn, id).unwrap().title)
            .collect();
        // 選んだ順（B, A）で保存され、A の後ろに置かれる
        assert_eq!(roots, ["A", "B", "A", "B"]);
    }

    #[test]
    fn rename_delete_and_errors() {
        let conn = db::open_in_memory().unwrap();
        let a = add(&conn, "A", None, Some("2026-01-01"), None);
        assert!(save_template(&conn, "  ", &[a]).is_err());
        assert!(save_template(&conn, "x", &[]).is_err());
        let s = save_template(&conn, "x", &[a]).unwrap();
        assert_eq!(s.span_days, Some(1));
        assert_eq!(rename_template(&conn, s.id, "y").unwrap().name, "y");
        assert!(
            apply_template(
                &conn,
                s.id,
                "2026/01/01",
                Placement::Into {
                    parent_id: None,
                    inbox: true
                }
            )
            .is_err()
        );
        delete_template(&conn, s.id).unwrap();
        assert!(list_templates(&conn).unwrap().is_empty());
        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM template_item", [], |r| r.get(0))
            .unwrap();
        assert_eq!(items, 0);
        assert!(delete_template(&conn, s.id).is_err());
    }

    #[test]
    fn undo_reverts_apply_and_delete() {
        let conn = db::open_in_memory().unwrap();
        let p = add(&conn, "P", None, Some("2026-01-05"), None);
        add(&conn, "C", Some(p), None, Some("2026-01-06"));
        let s = save_template(&conn, "t", &[p]).unwrap();
        let before = task_service::list_tasks(&conn).unwrap().len();

        undo_service::record(&conn, "テンプレートから作成", |c| {
            apply_template(
                c,
                s.id,
                "2026-02-02",
                Placement::Into {
                    parent_id: None,
                    inbox: false,
                },
            )
        })
        .unwrap();
        assert_eq!(task_service::list_tasks(&conn).unwrap().len(), before + 2);
        undo_service::undo(&conn).unwrap();
        assert_eq!(task_service::list_tasks(&conn).unwrap().len(), before);

        undo_service::record(&conn, "テンプレートの削除", |c| {
            delete_template(c, s.id)
        })
        .unwrap();
        assert!(list_templates(&conn).unwrap().is_empty());
        undo_service::undo(&conn).unwrap();
        let tpl = get_template(&conn, s.id).unwrap();
        assert_eq!(tpl.items[0].children[0].end_offset, Some(1));
    }
}
