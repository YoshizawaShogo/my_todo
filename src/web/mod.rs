//! 127.0.0.1 だけで待ち受ける HTTP サーバー。画面（静的ファイル）と REST API を配信する。

mod api;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rusqlite::Connection;

#[derive(Clone)]
pub struct AppState {
    /// 開いているデータベース。画面から別のファイルに切り替えられる
    pub conn: Arc<Mutex<Connection>>,
    /// 開いているデータベースの場所
    pub db_path: Arc<Mutex<std::path::PathBuf>>,
    /// Jira に送信中か（送信中はデータベースを切り替えない。送信の記録が別の DB に入らないように）
    pub jira_sending: Arc<std::sync::atomic::AtomicBool>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_html))
        .route("/{file}", get(static_file))
        .nest("/api", api::router())
        .layer(axum::middleware::from_fn(check_host_and_origin))
        .with_state(state)
}

/// 127.0.0.1 で待ち受けていても、ブラウザで開いた他のサイトから要求が届くことがある
/// （フォームでの POST や、DNS リバインディング）。Host が 127.0.0.1 か localhost の要求だけを受け付け、
/// Origin が付いているならこのアプリの画面からの要求だけを受け付ける
async fn check_host_and_origin(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let allowed = {
        let headers = req.headers();
        let header_str = |name| headers.get(name).and_then(|v| v.to_str().ok());
        let host_ok = header_str(header::HOST).is_some_and(is_local_host);
        let origin_ok = match header_str(header::ORIGIN) {
            None => true,
            Some(origin) => origin.strip_prefix("http://").is_some_and(is_local_host),
        };
        host_ok && origin_ok
    };
    if allowed {
        next.run(req).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "このアプリの画面からの要求だけを受け付けます",
        )
            .into_response()
    }
}

/// "127.0.0.1:8765" や "localhost:8765" か（ポートは問わない）
fn is_local_host(host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    matches!(name, "127.0.0.1" | "localhost")
}

pub async fn serve(
    conn: Connection,
    db_path: std::path::PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let url = format!("http://{addr}/");
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            // すでにこのアプリが起動しているなら、それをブラウザで開いて終わる（2 回目のダブルクリック）
            if is_my_todo_running(&url).await {
                println!("my_todo: すでに起動しています。ブラウザで {url} を開きます");
                if open_browser {
                    open_url(&url);
                    // ブラウザを開くコマンドが終わるのを少し待つ（すぐ終了すると開かないことがある）
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                return Ok(());
            }
            return Err(e).with_context(|| {
                format!("{addr} で待ち受けできません（ほかのアプリがポート {port} を使っているかもしれません。--port で別の番号を指定できます）")
            });
        }
    };
    let state = AppState {
        conn: Arc::new(Mutex::new(conn)),
        db_path: Arc::new(Mutex::new(crate::config::normalize(&db_path))),
        jira_sending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    // 先に要求を受け付け始め、ブラウザから実際に開けるようになってから案内する
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    });
    wait_until_reachable(&url).await;
    println!(
        "my_todo {}（ビルド {}）: {url} で起動しました（Ctrl+C で終了）",
        env!("CARGO_PKG_VERSION"),
        build_time_text()
    );
    if open_browser {
        open_url(&url);
    }
    server.await??;
    Ok(())
}

/// そのポートで動いているのがこのアプリか（/api/about が my_todo の形で返るか）
async fn is_my_todo_running(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy()
        .build()
    else {
        return false;
    };
    match client.get(format!("{url}api/about")).send().await {
        Ok(res) => res
            .json::<serde_json::Value>()
            .await
            .is_ok_and(|v| v.get("version").is_some() && v.get("db_path").is_some()),
        Err(_) => false,
    }
}

/// WSL では、Windows 側のブラウザから WSL 内のサーバーへの中継が、待ち受けを始めてから
/// 0.3〜1 秒ほど遅れて始まる。その前にブラウザを開くと「ページがありません」になるので、
/// Windows 側の curl.exe で実際につながるまで待つ（最大 10 秒）。WSL 以外では待たない
async fn wait_until_reachable(url: &str) {
    #[cfg(all(unix, not(target_os = "macos")))]
    if is_wsl() {
        let url = url.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < std::time::Duration::from_secs(10) {
                let ok = Command::new("curl.exe")
                    .args(["-s", "-o", "NUL", "--max-time", "1", &url])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                match ok {
                    Ok(s) if s.success() => return,
                    Ok(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                    // curl.exe が使えない環境では、中継が始まるくらいの時間だけ待つ
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        return;
                    }
                }
            }
        })
        .await;
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = url;
}

/// 既定のブラウザで開く。開けなければ URL を案内する。
/// 開くコマンドの出力は捨て、終了を待つのは別スレッドで行う。
fn open_url(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        let ok = browser_command(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            println!("my_todo: ブラウザで {url} を開いてください");
        }
    });
}

#[cfg(windows)]
fn browser_command(url: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "start", "", url]);
    cmd
}

#[cfg(target_os = "macos")]
fn browser_command(url: &str) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(url);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_command(url: &str) -> Command {
    if is_wsl() {
        // WSL では Windows 側の既定ブラウザで開く。
        // Linux 側のディレクトリのままだと cmd.exe が警告を出すので、C: に移ってから実行する
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "start", "", url]);
        if std::path::Path::new("/mnt/c").is_dir() {
            cmd.current_dir("/mnt/c");
        }
        cmd
    } else {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_lowercase().contains("microsoft"))
}

/// 画面のファイル。すべて .exe に埋め込む
const STATIC_FILES: &[(&str, &str, &str)] = &[
    (
        "alerts.js",
        "text/javascript",
        include_str!("static/alerts.js"),
    ),
    ("app.js", "text/javascript", include_str!("static/app.js")),
    (
        "database.js",
        "text/javascript",
        include_str!("static/database.js"),
    ),
    (
        "datepicker.js",
        "text/javascript",
        include_str!("static/datepicker.js"),
    ),
    (
        "gantt.js",
        "text/javascript",
        include_str!("static/gantt.js"),
    ),
    ("help.js", "text/javascript", include_str!("static/help.js")),
    ("jira.js", "text/javascript", include_str!("static/jira.js")),
    ("list.js", "text/javascript", include_str!("static/list.js")),
    ("work.js", "text/javascript", include_str!("static/work.js")),
    ("style.css", "text/css", include_str!("static/style.css")),
];

/// ビルドの印（ビルドした日時の UNIX 秒）。画面が古い版のままか見分けるのに使う
pub const BUILD: &str = env!("MY_TODO_BUILD");

async fn index_html() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // 古い画面をブラウザに覚えさせない（アプリを新しくしたら必ず新しい画面になるように）
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("static/index.html"),
    )
}

async fn static_file(Path(file): Path<String>) -> Response {
    match STATIC_FILES.iter().find(|(name, _, _)| *name == file) {
        Some((_, mime, body)) => (
            [
                (header::CONTENT_TYPE, format!("{mime}; charset=utf-8")),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// ビルドした日時（この PC の時刻で "2026-11-21 12:20"）
pub fn build_time_text() -> String {
    BUILD
        .parse::<i64>()
        .ok()
        .and_then(|s| jiff::Timestamp::from_second(s).ok())
        .map(|t| {
            t.to_zoned_datetime(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
