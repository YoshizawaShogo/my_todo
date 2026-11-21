//! 画面・MCP・Jira 連携のすべての入口から呼ばれるサービス層。
//! HTTP などの通信には依存しない。

pub mod agenda_service;
pub mod jira_service;
pub mod task_service;
pub mod undo_service;
pub mod work_service;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("タスクが見つかりません: {0}")]
    NotFound(i64),
    #[error("{0}")]
    Invalid(String),
    #[error("データベースエラー: {0}")]
    Db(#[from] rusqlite::Error),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

/// いまの日時（この PC の時差付き、秒まで）。例：2026-11-20T22:07:37+09:00
pub(crate) fn now_text() -> String {
    jiff::ZonedDateTime::now()
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// 今日（この PC の時差で）
pub fn today() -> jiff::civil::Date {
    jiff::ZonedDateTime::now().date()
}

/// YYYY-MM-DD の日付を読む（2026-1-5 や 11/30 のような形は受け付けない）
pub fn parse_date(s: &str) -> ServiceResult<jiff::civil::Date> {
    s.parse::<jiff::civil::Date>()
        .ok()
        .filter(|_| s.len() == 10)
        .ok_or_else(|| ServiceError::Invalid(format!("日付は YYYY-MM-DD で指定してください: {s}")))
}

/// 入れ子にできるトランザクション（SQLite のセーブポイント）。
/// 単独で使えば BEGIN と同じ。外側に別のトランザクションがあれば、その一部になる。
/// commit しないまま捨てると、その中の変更をすべて取り消す。
pub(crate) struct Tx<'a> {
    conn: &'a rusqlite::Connection,
    done: bool,
}

impl<'a> Tx<'a> {
    pub fn begin(conn: &'a rusqlite::Connection) -> rusqlite::Result<Self> {
        conn.execute_batch("SAVEPOINT svc")?;
        Ok(Tx { conn, done: false })
    }

    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.done = true;
        self.conn.execute_batch("RELEASE svc")
    }
}

impl std::ops::Deref for Tx<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &rusqlite::Connection {
        self.conn
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch("ROLLBACK TO svc; RELEASE svc");
        }
    }
}
