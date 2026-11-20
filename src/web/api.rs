use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::AppState;
use crate::model::{MoveTarget, NewTask, TagCount, Task, TaskEvent, TaskPatch};
use crate::service::agenda_service::{self, Agenda};
use crate::service::work_service::{self, DaySummary, SaveDay, Settings, SettingsPatch, WorkDay};
use crate::service::{ServiceError, task_service, undo_service};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list_tasks).post(create_task))
        .route(
            "/tasks/{id}",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/tasks/{id}/move", post(move_task))
        .route("/tasks/{id}/events", get(list_events))
        .route("/agenda", get(get_agenda))
        .route("/tags", get(list_tags))
        .route("/work_days", get(list_work_days))
        .route("/work_days/{date}", get(get_work_day).put(save_work_day))
        .route("/settings", get(get_settings).patch(update_settings))
        .route("/undo", post(undo))
        .route("/redo", post(redo))
}

struct ApiError(ServiceError);

impl From<ServiceError> for ApiError {
    fn from(e: ServiceError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            ServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            ServiceError::Invalid(_) => StatusCode::BAD_REQUEST,
            ServiceError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn with_conn<T>(
    state: &AppState,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, ServiceError>,
) -> Result<T, ApiError> {
    let conn = state.conn.lock().unwrap_or_else(|e| e.into_inner());
    Ok(f(&conn)?)
}

async fn list_tasks(State(s): State<AppState>) -> ApiResult<Vec<Task>> {
    with_conn(&s, task_service::list_tasks).map(Json)
}

async fn get_task(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Task> {
    with_conn(&s, |c| task_service::get_task(c, id)).map(Json)
}

// 変更系はすべて undo_service::record で包み、Ctrl+Z で元に戻せるようにする

async fn create_task(State(s): State<AppState>, Json(input): Json<NewTask>) -> ApiResult<Task> {
    let label = format!("「{}」の追加", input.title.trim());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| task_service::create_task(c, input))
    })
    .map(Json)
}

/// 何を変えたかを元に戻すときの表示用に短く言う
fn describe_patch(p: &TaskPatch) -> &'static str {
    if p.title.is_some() {
        "名前の変更"
    } else if p.status.is_some() {
        "状態の変更"
    } else if p.planned_start.is_some() || p.planned_end.is_some() {
        "日程の変更"
    } else if p.next_action.is_some() || p.next_action_date.is_some() {
        "次の予定の変更"
    } else if p.tags.is_some() {
        "タグの変更"
    } else if p.estimate_min.is_some() {
        "見積もりの変更"
    } else if p.note.is_some() {
        "メモの変更"
    } else {
        "変更"
    }
}

async fn update_task(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(patch): Json<TaskPatch>,
) -> ApiResult<Task> {
    with_conn(&s, |c| {
        let label = format!(
            "「{}」の{}",
            task_service::get_task(c, id)?.title,
            describe_patch(&patch)
        );
        undo_service::record(c, &label, |c| task_service::update_task(c, id, patch))
    })
    .map(Json)
}

async fn move_task(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<MoveTarget>,
) -> ApiResult<Task> {
    with_conn(&s, |c| {
        let label = format!("「{}」の移動", task_service::get_task(c, id)?.title);
        undo_service::record(c, &label, |c| task_service::move_task(c, id, target))
    })
    .map(Json)
}

async fn delete_task(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    with_conn(&s, |c| {
        let label = format!("「{}」の削除", task_service::get_task(c, id)?.title);
        undo_service::record(c, &label, |c| task_service::delete_task(c, id))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Serialize)]
struct UndoResult {
    /// 元に戻した（やり直した）操作の名前。何もなければ null
    label: Option<String>,
}

async fn undo(State(s): State<AppState>) -> ApiResult<UndoResult> {
    with_conn(&s, undo_service::undo).map(|label| Json(UndoResult { label }))
}

async fn redo(State(s): State<AppState>) -> ApiResult<UndoResult> {
    with_conn(&s, undo_service::redo).map(|label| Json(UndoResult { label }))
}

async fn list_events(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Vec<TaskEvent>> {
    with_conn(&s, |c| task_service::list_events(c, id)).map(Json)
}

#[derive(serde::Deserialize)]
struct AgendaQuery {
    /// ブラウザ側の今日（YYYY-MM-DD）。省略時はサーバーの今日
    today: Option<String>,
}

async fn get_agenda(State(s): State<AppState>, Query(q): Query<AgendaQuery>) -> ApiResult<Agenda> {
    let today = match q.today.as_deref() {
        Some(d) => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
            ServiceError::Invalid(format!("日付は YYYY-MM-DD で指定してください: {d}"))
        })?,
        None => chrono::Local::now().date_naive(),
    };
    with_conn(&s, |c| agenda_service::agenda(c, today)).map(Json)
}

async fn list_tags(State(s): State<AppState>) -> ApiResult<Vec<TagCount>> {
    with_conn(&s, task_service::list_tags).map(Json)
}

// ---------- 工数 ----------
#[derive(serde::Deserialize)]
struct RangeQuery {
    from: String,
    to: String,
}

async fn list_work_days(
    State(s): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Vec<DaySummary>> {
    with_conn(&s, |c| work_service::list_days(c, &q.from, &q.to)).map(Json)
}

async fn get_work_day(State(s): State<AppState>, Path(date): Path<String>) -> ApiResult<WorkDay> {
    with_conn(&s, |c| work_service::get_day(c, &date)).map(Json)
}

async fn save_work_day(
    State(s): State<AppState>,
    Path(date): Path<String>,
    Json(input): Json<SaveDay>,
) -> ApiResult<WorkDay> {
    let label = format!("{date} の工数");
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| work_service::save_day(c, &date, input))
    })
    .map(Json)
}

async fn get_settings(State(s): State<AppState>) -> ApiResult<Settings> {
    with_conn(&s, work_service::get_settings).map(Json)
}

async fn update_settings(
    State(s): State<AppState>,
    Json(patch): Json<SettingsPatch>,
) -> ApiResult<Settings> {
    with_conn(&s, |c| {
        undo_service::record(c, "設定の変更", |c| {
            work_service::update_settings(c, patch)
        })
    })
    .map(Json)
}
