use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::AppState;
use crate::model::{MoveTarget, NewTask, Placement, TagCount, Task, TaskEvent, TaskPatch};
use crate::service::agenda_service::{self, Agenda};
use crate::service::template_service::{self, Template, TemplateSummary};
use crate::service::work_service::{self, DaySummary, SaveDay, Settings, SettingsPatch, WorkDay};
use crate::service::{ServiceError, link_service, task_service, undo_service};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list_tasks).post(create_task))
        .route(
            "/tasks/{id}",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/tasks/{id}/move", post(move_task))
        .route("/tasks/move_many", post(move_tasks))
        .route("/tasks/copy", post(copy_tasks))
        .route("/tasks/delete_many", post(delete_tasks))
        .route("/tasks/add_tag", post(add_tag))
        .route("/tags/rename", post(rename_tag))
        .route("/tasks/create_many", post(create_tasks))
        .route("/tasks/{id}/events", get(list_events))
        .route("/tasks/{id}/links", post(add_link))
        .route(
            "/links/{id}",
            axum::routing::patch(rename_link).delete(delete_link),
        )
        .route("/agenda", get(get_agenda))
        .route("/tags", get(list_tags))
        .route("/work_days", get(list_work_days))
        .route("/work_days/{date}", get(get_work_day).put(save_work_day))
        .route("/settings", get(get_settings).patch(update_settings))
        .route("/jira", get(get_jira).patch(update_jira))
        .route("/jira/test", post(test_jira))
        .route("/jira/issues/{key}", get(get_jira_issue))
        .route("/jira/draft", get(get_jira_draft))
        .route("/jira/search", get(search_jira))
        .route("/jira/import", post(import_jira))
        .route("/jira/send", post(send_jira))
        .route("/about", get(about))
        .route("/databases", get(list_databases))
        .route("/databases/open", post(open_database))
        .route("/databases/forget", post(forget_database))
        .route("/databases/pick", post(pick_database))
        .route("/templates", get(list_templates).post(save_template))
        .route(
            "/templates/{id}",
            get(get_template)
                .patch(rename_template)
                .delete(delete_template),
        )
        .route("/templates/{id}/apply", post(apply_template))
        .route("/bye", post(bye))
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
    } else if p.jira_key.is_some() {
        "Jira 課題の紐付け"
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

/// 画面が閉じられた（ページの pagehide から navigator.sendBeacon で届く）。
/// しばらくだれもつながってこなければアプリを終える
async fn bye(State(s): State<AppState>) -> StatusCode {
    s.activity.closed();
    StatusCode::NO_CONTENT
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
        Some(d) => crate::service::parse_date(d)?,
        None => crate::service::today(),
    };
    with_conn(&s, |c| agenda_service::agenda(c, today)).map(Json)
}

async fn list_tags(State(s): State<AppState>) -> ApiResult<Vec<TagCount>> {
    with_conn(&s, task_service::list_tags).map(Json)
}

// ---------- 工数 ----------
#[derive(serde::Deserialize)]
pub struct RangeQuery {
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

// ---------- Jira ----------
use crate::jira::{self, DataCenterClient, JiraApi};
use crate::service::jira_service;

/// Jira とのやり取りの失敗は 502 で、理由をそのまま返す
fn jira_error(msg: String) -> Response {
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response()
}

fn jira_client(s: &AppState) -> Result<DataCenterClient, String> {
    let base = with_conn(s, jira_service::get_config)
        .map_err(|e| e.0.to_string())?
        .base_url
        .ok_or("Jira の URL が設定されていません（Jira タブで設定）")?;
    let (token, _) = jira::load_token(Some(&base))
        .ok_or("この URL 用の PAT が設定されていません（Jira タブで設定）")?;
    DataCenterClient::new(&base, &token)
}

async fn get_jira(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let config = with_conn(&s, jira_service::get_config)?;
    let source = jira::load_token(config.base_url.as_deref()).map(|(_, src)| src);
    Ok(Json(
        json!({ "base_url": config.base_url, "token_source": source }),
    ))
}

#[derive(serde::Deserialize)]
struct JiraConfigPatch {
    base_url: Option<String>,
    /// 空文字で削除。値は返さない
    token: Option<String>,
}

async fn update_jira(State(s): State<AppState>, Json(p): Json<JiraConfigPatch>) -> Response {
    if let Some(url) = &p.base_url
        && let Err(e) = with_conn(&s, |c| jira_service::set_base_url(c, url))
    {
        return e.into_response();
    }
    let base_url = match with_conn(&s, jira_service::get_config) {
        Ok(c) => c.base_url,
        Err(e) => return e.into_response(),
    };
    if let Some(token) = &p.token
        && let Err(e) = jira::save_token(base_url.as_deref(), token)
    {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    match get_jira(State(s)).await {
        Ok(j) => j.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn test_jira(State(s): State<AppState>) -> Response {
    let client = match jira_client(&s) {
        Ok(c) => c,
        Err(e) => return jira_error(e),
    };
    match client.myself().await {
        Ok(me) => Json(me).into_response(),
        Err(e) => jira_error(e),
    }
}

async fn get_jira_issue(State(s): State<AppState>, Path(key): Path<String>) -> Response {
    let key = match task_service::normalize_issue_key(Some(key)) {
        Ok(Some(k)) => k,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return ApiError(e).into_response(),
    };
    let client = match jira_client(&s) {
        Ok(c) => c,
        Err(e) => return jira_error(e),
    };
    match client.issue(&key).await {
        Ok(i) => Json(i).into_response(),
        Err(e) => jira_error(e),
    }
}

async fn get_jira_draft(
    State(s): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<jira_service::Draft> {
    with_conn(&s, |c| jira_service::build_draft(c, &q.from, &q.to)).map(Json)
}

#[derive(serde::Deserialize)]
struct SendRequest {
    from: String,
    to: String,
    /// 画面で確認した送る内容（作り直した下書きと食い違ったら送らない）
    expected: Vec<jira::ExpectedLine>,
}

/// 送信中の印。送り終えたら（途中で失敗・パニックしても）必ず下ろす
struct SendingGuard(Arc<AtomicBool>);

impl Drop for SendingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn send_jira(State(s): State<AppState>, Json(q): Json<SendRequest>) -> Response {
    let client = match jira_client(&s) {
        Ok(c) => c,
        Err(e) => return jira_error(e),
    };
    // 送信中の印を立てる（同時に 2 回送らない・送信中は DB を切り替えさせない）
    if s.jira_sending
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "すでに Jira に送信中です" })),
        )
            .into_response();
    }
    let guard = SendingGuard(s.jira_sending.clone());
    // 画面を閉じる・読み直すと、この要求の処理は途中で打ち切られる。送信と記録が途中で止まると
    // 二重送信のもとになるので、別のタスクで最後まで送る
    let conn = s.conn.clone();
    let task = tokio::spawn(async move {
        let _guard = guard;
        jira::send_draft(&client, &conn, &q.from, &q.to, &q.expected).await
    });
    match task.await {
        Ok(Ok(results)) => Json(results).into_response(),
        Ok(Err(e)) if e == jira::CHANGED_SINCE_CONFIRM => {
            (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response()
        }
        Ok(Err(e)) => jira_error(e),
        Err(e) => jira_error(format!("送信が途中で止まりました: {e}")),
    }
}

/// アプリの版と開いている DB（起動済みかの確認・新しい版の検知に使う）
async fn about(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build": super::build_id(),
        "db_path": plain_path(&current_db(&s)),
        "windows": cfg!(windows),
    }))
}

/// Windows の絶対パスに付く \\?\ を外す（設定ファイルに貼りやすいように）
fn plain_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(String::from).unwrap_or(s)
}

// ---------- データベースの切り替え ----------
use crate::config::{self, Config};

fn current_db(s: &AppState) -> std::path::PathBuf {
    s.db_path.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(serde::Serialize)]
struct DbEntry {
    path: String,
    name: String,
    exists: bool,
    current: bool,
}

/// 開いているデータベースと、最近開いたデータベースの一覧
async fn list_databases(State(s): State<AppState>) -> Json<serde_json::Value> {
    let current = current_db(&s);
    let entry = |p: &std::path::Path| DbEntry {
        path: plain_path(p),
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        exists: p.is_file(),
        current: config::normalize(p) == current,
    };
    let recent: Vec<DbEntry> = Config::load().recent_dbs.iter().map(|p| entry(p)).collect();
    // can_pick：エクスプローラーの画面でファイルを選べるか（Windows だけ）
    Json(json!({ "current": entry(&current), "recent": recent, "can_pick": cfg!(windows) }))
}

/// エクスプローラーの画面（ファイルを開くダイアログ）でデータベースを選ぶ。選んだ場所を返すだけで、開くのは画面側
async fn pick_database(State(s): State<AppState>) -> Response {
    static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if PICKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "ファイルを選ぶ画面がすでに開いています（タスクバーを確認してください）" })),
        )
            .into_response();
    }
    let current = current_db(&s);
    // ダイアログは閉じるまで戻らないので、非同期の処理を止めないよう別のスレッドで出す
    let picked =
        tokio::task::spawn_blocking(move || crate::file_dialog::pick_database(Some(&current)))
            .await;
    PICKING.store(false, std::sync::atomic::Ordering::SeqCst);
    let path = picked.ok().flatten().map(|p| plain_path(&p));
    Json(json!({ "path": path })).into_response()
}

#[derive(serde::Deserialize)]
struct OpenDb {
    path: String,
    /// ファイルがなければ新しく作る
    #[serde(default)]
    create: bool,
}

/// 別のデータベースを開いて切り替える
async fn open_database(State(s): State<AppState>, Json(req): Json<OpenDb>) -> Response {
    if s.jira_sending.load(std::sync::atomic::Ordering::SeqCst) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Jira に送信中です。終わってから切り替えてください" })),
        )
            .into_response();
    }
    let raw = req.path.trim().trim_matches('"');
    if raw.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "ファイルの場所を入れてください" })),
        )
            .into_response();
    }
    let mut path = std::path::PathBuf::from(raw);
    // フォルダを指定されたら、その中の my_todo.db
    if path.is_dir() {
        path = path.join("my_todo.db");
    }
    if !path.is_file() && !req.create {
        // 画面側で「新しく作りますか？」と聞けるように、専用の印を付けて返す
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("ファイルがありません: {}", plain_path(&path)), "missing": true, "path": plain_path(&path) })),
        )
            .into_response();
    }
    if req.create
        && let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && !dir.is_dir()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("フォルダがありません: {}", plain_path(dir)) })),
        )
            .into_response();
    }
    // 開けるか（SQLite のファイルか、マイグレーションできるか）を確かめてから差し替える
    let conn = match crate::db::open(&path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("このファイルは開けません（my_todo のデータベースではないかもしれません）: {e:#}") })),
            )
                .into_response();
        }
    };
    let normalized = config::normalize(&path);
    *s.conn.lock().unwrap_or_else(|e| e.into_inner()) = conn;
    *s.db_path.lock().unwrap_or_else(|e| e.into_inner()) = normalized.clone();
    config::remember_db(&normalized);
    println!(
        "my_todo: データベースを切り替えました: {}",
        plain_path(&normalized)
    );
    list_databases(State(s)).await.into_response()
}

#[derive(serde::Deserialize)]
struct ForgetDb {
    path: String,
}

/// 最近開いたファイルの一覧から外す（ファイル自体は消さない）
async fn forget_database(
    State(s): State<AppState>,
    Json(req): Json<ForgetDb>,
) -> Json<serde_json::Value> {
    let mut c = Config::load();
    c.forget(std::path::Path::new(&req.path));
    let _ = c.save();
    list_databases(State(s)).await
}

#[derive(serde::Deserialize)]
struct JiraSearchQuery {
    jql: String,
}

/// JQL で Jira の課題を探す（紐付けの右側の一覧）
async fn search_jira(State(s): State<AppState>, Query(q): Query<JiraSearchQuery>) -> Response {
    let client = match jira_client(&s) {
        Ok(c) => c,
        Err(e) => return jira_error(e),
    };
    match client.search(&q.jql, 200).await {
        Ok(issues) => Json(issues).into_response(),
        Err(e) => jira_error(e),
    }
}

#[derive(serde::Deserialize)]
struct ImportIssue {
    key: String,
    summary: String,
}

#[derive(serde::Deserialize)]
struct JiraImport {
    issues: Vec<ImportIssue>,
}

/// Jira の課題を、課題キー付きのタスクとして未分類に取り込む（まとめて 1 回の操作）
async fn import_jira(
    State(s): State<AppState>,
    Json(req): Json<JiraImport>,
) -> ApiResult<Vec<Task>> {
    let label = format!("Jira から {} 件の取り込み", req.issues.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            let mut out = Vec::new();
            for i in req.issues {
                let title = if i.summary.trim().is_empty() {
                    i.key.clone()
                } else {
                    i.summary
                };
                let t = task_service::create_task(
                    c,
                    NewTask {
                        title,
                        parent_id: None,
                        note: None,
                        inbox: Some(true),
                    },
                )?;
                let patch = TaskPatch {
                    jira_key: Some(Some(i.key)),
                    ..Default::default()
                };
                out.push(task_service::update_task(c, t.id, patch)?);
            }
            Ok(out)
        })
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
struct MoveMany {
    /// 画面での並び順
    ids: Vec<i64>,
    placement: Placement,
}

/// 複数のタスクをまとめて移動する（1 回の操作として元に戻せる）
async fn move_tasks(State(s): State<AppState>, Json(req): Json<MoveMany>) -> ApiResult<Vec<Task>> {
    let label = format!("{} 件の移動", req.ids.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            task_service::move_tasks(c, &req.ids, req.placement)
        })
    })
    .map(Json)
}

/// タスクを子孫ごと複製して貼り付ける（1 回の操作として元に戻せる）
async fn copy_tasks(State(s): State<AppState>, Json(req): Json<MoveMany>) -> ApiResult<Vec<Task>> {
    let label = format!("{} 件の貼り付け", req.ids.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            task_service::copy_tasks(c, &req.ids, req.placement)
        })
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
struct CreateMany {
    titles: Vec<String>,
    placement: Placement,
}

/// 複数行の文字から 1 行 1 タスクで作る（ほかのアプリからの貼り付け）
async fn create_tasks(
    State(s): State<AppState>,
    Json(req): Json<CreateMany>,
) -> ApiResult<Vec<Task>> {
    let label = format!("{} 行の貼り付け", req.titles.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            task_service::create_tasks(c, &req.titles, req.placement)
        })
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
struct AddTag {
    ids: Vec<i64>,
    tag: String,
}

#[derive(serde::Serialize)]
struct AddTagResult {
    /// タグが新しく付いた件数（すでに付いていたものは数えない）
    changed: usize,
}

/// 複数のタスクにタグを 1 つ足す（タグをタスクへドラッグしたとき。1 回の操作として元に戻せる）
async fn add_tag(State(s): State<AppState>, Json(req): Json<AddTag>) -> ApiResult<AddTagResult> {
    let label = format!("タグ「{}」を {} 件に追加", req.tag.trim(), req.ids.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| task_service::add_tag(c, &req.ids, &req.tag))
    })
    .map(|changed| Json(AddTagResult { changed }))
}

#[derive(serde::Deserialize)]
struct RenameTag {
    from: String,
    to: String,
}

/// タグの名前を変える（すべてのタスクで。1 回の操作として元に戻せる）
async fn rename_tag(
    State(s): State<AppState>,
    Json(req): Json<RenameTag>,
) -> ApiResult<AddTagResult> {
    let label = format!("タグ「{}」を「{}」に変更", req.from.trim(), req.to.trim());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            task_service::rename_tag(c, &req.from, &req.to)
        })
    })
    .map(|changed| Json(AddTagResult { changed }))
}

#[derive(serde::Deserialize)]
struct DeleteMany {
    ids: Vec<i64>,
}

/// 複数のタスクを子孫ごとまとめて削除する（1 回の操作として元に戻せる）
async fn delete_tasks(
    State(s): State<AppState>,
    Json(req): Json<DeleteMany>,
) -> Result<StatusCode, ApiError> {
    let label = format!("{} 件の削除", req.ids.len());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| task_service::delete_tasks(c, &req.ids))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- 関連資料 ----------

#[derive(serde::Deserialize)]
struct NewLink {
    url: String,
    /// 省略するとリンク先のページの名前（取れなければホスト名とパス）
    title: Option<String>,
}

/// 関連資料を足す。名前はリンク先のページから取る（データベースのロックは取らずに通信する）
async fn add_link(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<NewLink>,
) -> ApiResult<Task> {
    let url = link_service::normalize_url(&req.url)?;
    let title = match req.title.filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => crate::link_title::fetch_title(&url)
            .await
            .unwrap_or_default(),
    };
    with_conn(&s, |c| {
        undo_service::record(c, "関連資料の追加", |c| {
            link_service::add_link(c, id, &url, &title)
        })?;
        task_service::get_task(c, id)
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
struct RenameLink {
    title: String,
}

async fn rename_link(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<RenameLink>,
) -> ApiResult<crate::model::TaskLink> {
    with_conn(&s, |c| {
        undo_service::record(c, "関連資料の名前の変更", |c| {
            link_service::rename_link(c, id, &req.title)
        })
    })
    .map(Json)
}

async fn delete_link(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    with_conn(&s, |c| {
        undo_service::record(c, "関連資料の削除", |c| {
            link_service::delete_link(c, id)
        })
    })?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- テンプレート ----------

async fn list_templates(State(s): State<AppState>) -> ApiResult<Vec<TemplateSummary>> {
    with_conn(&s, template_service::list_templates).map(Json)
}

async fn get_template(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Template> {
    with_conn(&s, |c| template_service::get_template(c, id)).map(Json)
}

#[derive(serde::Deserialize)]
struct SaveTemplate {
    name: String,
    ids: Vec<i64>,
}

async fn save_template(
    State(s): State<AppState>,
    Json(req): Json<SaveTemplate>,
) -> ApiResult<TemplateSummary> {
    let label = format!("テンプレート『{}』の保存", req.name.trim());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            template_service::save_template(c, &req.name, &req.ids)
        })
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
struct RenameTemplate {
    name: String,
}

async fn rename_template(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<RenameTemplate>,
) -> ApiResult<TemplateSummary> {
    let label = format!("テンプレートの名前を『{}』に変更", req.name.trim());
    with_conn(&s, |c| {
        undo_service::record(c, &label, |c| {
            template_service::rename_template(c, id, &req.name)
        })
    })
    .map(Json)
}

async fn delete_template(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    with_conn(&s, |c| {
        let label = format!(
            "テンプレート『{}』の削除",
            template_service::get_template(c, id)?.summary.name
        );
        undo_service::record(c, &label, |c| template_service::delete_template(c, id))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct ApplyTemplate {
    start_date: String,
    placement: Placement,
}

#[derive(serde::Serialize)]
struct Applied {
    /// 作ったタスクの数（子を含む）
    created: usize,
}

async fn apply_template(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ApplyTemplate>,
) -> ApiResult<Applied> {
    with_conn(&s, |c| {
        let label = format!(
            "テンプレート『{}』から作成",
            template_service::get_template(c, id)?.summary.name
        );
        undo_service::record(c, &label, |c| {
            template_service::apply_template(c, id, &req.start_date, req.placement)
        })
    })
    .map(|created| Json(Applied { created }))
}
