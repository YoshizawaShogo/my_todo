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
    pub conn: Arc<Mutex<Connection>>,
    /// ヘルプに出す（MCP の設定例に使う）
    pub db_path: Arc<std::path::PathBuf>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_html))
        .route("/{file}", get(static_file))
        .nest("/api", api::router())
        .with_state(state)
}

pub async fn serve(
    conn: Connection,
    db_path: std::path::PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("{addr} で待ち受けできません（ポートが使用中かもしれません）"))?;
    let state = AppState {
        conn: Arc::new(Mutex::new(conn)),
        db_path: Arc::new(std::fs::canonicalize(&db_path).unwrap_or(db_path)),
    };
    // 先に要求を受け付け始め、ブラウザから実際に開けるようになってから案内する
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    });
    let url = format!("http://{addr}/");
    wait_until_reachable(&url).await;
    println!("my_todo: {url} で起動しました（Ctrl+C で終了）");
    if open_browser {
        open_url(&url);
    }
    server.await??;
    Ok(())
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
    ("app.js", "text/javascript", include_str!("static/app.js")),
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

async fn index_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("static/index.html"),
    )
}

async fn static_file(Path(file): Path<String>) -> Response {
    match STATIC_FILES.iter().find(|(name, _, _)| *name == file) {
        Some((_, mime, body)) => (
            [(header::CONTENT_TYPE, format!("{mime}; charset=utf-8"))],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
