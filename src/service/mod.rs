//! 画面・MCP・Jira 連携のすべての入口から呼ばれるサービス層。
//! HTTP などの通信には依存しない。

pub mod agenda_service;
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

pub(crate) fn now_text() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}
