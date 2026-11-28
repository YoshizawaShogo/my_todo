//! 画面・MCP・Jira 連携のすべての入口から呼ばれるサービス層。
//! HTTP などの通信には依存しない。

pub mod agenda_service;
pub mod calendar;
pub mod jira_service;
pub mod link_service;
pub mod review_service;
pub mod task_service;
pub mod template_service;
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

/// 入れ子にできるトランザクション。
/// 一番外側は `BEGIN IMMEDIATE`（最初から書き込みのロックを取る）。画面と MCP の 2 つのプロセスが
/// 同じファイルに書くとき、読んでから書く形（DEFERRED）だと書き込みへの切り替えが待たずに
/// SQLITE_BUSY で失敗するため。内側は SAVEPOINT で、外側のトランザクションの一部になる。
/// commit しないまま捨てると、その中の変更をすべて取り消す。
pub(crate) struct Tx<'a> {
    conn: &'a rusqlite::Connection,
    kind: TxKind,
    done: bool,
}

#[derive(Clone, Copy)]
enum TxKind {
    /// 一番外側（BEGIN IMMEDIATE … COMMIT / ROLLBACK）
    Outer,
    /// 入れ子（SAVEPOINT … RELEASE / ROLLBACK TO）
    Nested,
}

impl<'a> Tx<'a> {
    pub fn begin(conn: &'a rusqlite::Connection) -> rusqlite::Result<Self> {
        let kind = if conn.is_autocommit() {
            TxKind::Outer
        } else {
            TxKind::Nested
        };
        conn.execute_batch(match kind {
            TxKind::Outer => "BEGIN IMMEDIATE",
            TxKind::Nested => "SAVEPOINT svc",
        })?;
        Ok(Tx {
            conn,
            kind,
            done: false,
        })
    }

    /// 確定する。失敗したとき（外部キーの遅延確認で弾かれたときなど）は SQLite がトランザクションを
    /// 開いたままにするので、done を立てずに Drop で必ず ROLLBACK させる（開きっぱなしにすると、
    /// その接続のその後の変更がすべて入れ子扱いになって確定されず、書き込みのロックも取られたままになる）
    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(match self.kind {
            TxKind::Outer => "COMMIT",
            TxKind::Nested => "RELEASE svc",
        })?;
        self.done = true;
        Ok(())
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
            let _ = self.conn.execute_batch(match self.kind {
                TxKind::Outer => "ROLLBACK",
                TxKind::Nested => "ROLLBACK TO svc; RELEASE svc",
            });
        }
    }
}

str_enum! {
    /// setting テーブルのキー
    pub enum SettingKey {
        /// 1D（1 人日）の時間（分）
        MinutesPerDay => "minutes_per_day",
        /// 新しい日の勤務時間の初期値（分）
        DefaultWorkMin => "default_work_min",
        /// Jira の URL
        JiraBaseUrl => "jira_base_url",
    }
}

pub(crate) fn get_setting(
    conn: &rusqlite::Connection,
    key: SettingKey,
) -> ServiceResult<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT value FROM setting WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub(crate) fn set_setting(
    conn: &rusqlite::Connection,
    key: SettingKey,
    value: &str,
) -> ServiceResult<()> {
    conn.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}
