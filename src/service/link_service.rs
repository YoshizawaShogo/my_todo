//! 関連資料：タスクに付ける URL と表示名。

use rusqlite::{Connection, OptionalExtension, params};

use super::{ServiceError, ServiceResult, Tx, now_text, task_service};
use crate::model::TaskLink;

const MAX_URL_LEN: usize = 2048;
const MAX_TITLE_CHARS: usize = 200;

/// http:// か https:// の URL だけを受け付ける（javascript: などをリンクにしない）
pub fn normalize_url(url: &str) -> ServiceResult<String> {
    let url = url.trim();
    if url.len() > MAX_URL_LEN {
        return Err(ServiceError::Invalid("URL が長すぎます".into()));
    }
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| ServiceError::Invalid(format!("URL として読めません: {url}")))?;
    match parsed.scheme() {
        "http" | "https" if parsed.host_str().is_some() => Ok(parsed.to_string()),
        _ => Err(ServiceError::Invalid(
            "関連資料には http:// か https:// の URL を貼ってください".into(),
        )),
    }
}

/// 表示名がないときの名前：ホスト名とパス（長ければ途中を省く）
pub fn fallback_title(url: &str) -> String {
    let Ok(u) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    let host = u.host_str().unwrap_or("");
    let path = u.path().trim_end_matches('/');
    let text = format!("{host}{path}");
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 60 {
        text
    } else {
        let head: String = chars[..40].iter().collect();
        let tail: String = chars[chars.len() - 15..].iter().collect();
        format!("{head}…{tail}")
    }
}

fn normalize_title(title: &str, url: &str) -> String {
    let t: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(MAX_TITLE_CHARS).collect();
    if t.is_empty() { fallback_title(url) } else { t }
}

pub fn add_link(
    conn: &Connection,
    task_id: i64,
    url: &str,
    title: &str,
) -> ServiceResult<TaskLink> {
    let url = normalize_url(url)?;
    let title = normalize_title(title, &url);
    let tx = Tx::begin(conn)?;
    task_service::get_task(&tx, task_id)?;
    let order: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM task_link WHERE task_id = ?1",
        [task_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO task_link (task_id, sort_order, url, title, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, order, url, title, now_text()],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(TaskLink { id, url, title })
}

fn get_link(conn: &Connection, id: i64) -> ServiceResult<(i64, TaskLink)> {
    conn.query_row(
        "SELECT task_id, id, url, title FROM task_link WHERE id = ?1",
        [id],
        |r| {
            Ok((
                r.get(0)?,
                TaskLink {
                    id: r.get(1)?,
                    url: r.get(2)?,
                    title: r.get(3)?,
                },
            ))
        },
    )
    .optional()?
    .ok_or_else(|| ServiceError::Invalid(format!("関連資料 {id} がありません")))
}

/// 表示名を変える（空にするとホスト名とパスに戻る）
pub fn rename_link(conn: &Connection, id: i64, title: &str) -> ServiceResult<TaskLink> {
    let tx = Tx::begin(conn)?;
    let (_, link) = get_link(&tx, id)?;
    let title = normalize_title(title, &link.url);
    tx.execute(
        "UPDATE task_link SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    tx.commit()?;
    Ok(TaskLink { title, ..link })
}

/// 削除して、そのタスクの id を返す
pub fn delete_link(conn: &Connection, id: i64) -> ServiceResult<i64> {
    let tx = Tx::begin(conn)?;
    let (task_id, _) = get_link(&tx, id)?;
    tx.execute("DELETE FROM task_link WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(task_id)
}

/// タスクを複製したときに、関連資料も写す
pub fn copy_links(conn: &Connection, from: i64, to: i64) -> ServiceResult<()> {
    conn.execute(
        "INSERT INTO task_link (task_id, sort_order, url, title, created_at) \
         SELECT ?2, sort_order, url, title, ?3 FROM task_link WHERE task_id = ?1 ORDER BY sort_order, id",
        params![from, to, now_text()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::NewTask;
    use crate::service::undo_service;

    fn task(conn: &Connection) -> i64 {
        task_service::create_task(
            conn,
            NewTask {
                title: "t".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn add_rename_delete_and_undo() {
        let conn = db::open_in_memory().unwrap();
        let t = task(&conn);
        let l = undo_service::record(&conn, "追加", |c| {
            add_link(c, t, " https://example.com/docs/a ", "  設計  書 ")
        })
        .unwrap();
        assert_eq!(l.title, "設計 書");
        assert_eq!(
            task_service::get_task(&conn, t).unwrap().links,
            std::slice::from_ref(&l)
        );
        // 空にするとホスト名とパス
        assert_eq!(
            rename_link(&conn, l.id, " ").unwrap().title,
            "example.com/docs/a"
        );
        undo_service::record(&conn, "削除", |c| delete_link(c, l.id)).unwrap();
        assert!(task_service::get_task(&conn, t).unwrap().links.is_empty());
        undo_service::undo(&conn).unwrap();
        assert_eq!(task_service::get_task(&conn, t).unwrap().links.len(), 1);
        // 一覧にも入る
        let all = task_service::list_tasks(&conn).unwrap();
        assert_eq!(all[0].links.len(), 1);
    }

    #[test]
    fn only_http_urls() {
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("file:///C:/x.txt").is_err());
        assert!(normalize_url("example.com").is_err());
        assert_eq!(
            normalize_url("https://example.com").unwrap(),
            "https://example.com/"
        );
    }

    #[test]
    fn long_fallback_is_shortened() {
        let url = format!("https://example.com/{}", "a".repeat(100));
        let t = fallback_title(&url);
        assert!(t.chars().count() < 60 && t.contains('…'), "{t}");
    }
}
