// Windows では窓（コンソール）を出さないアプリとして作る。画面はブラウザで開き、
// 画面を閉じると自動で終わる。エラーはダイアログで知らせる
#![cfg_attr(windows, windows_subsystem = "windows")]

#[macro_use]
mod str_enum;

mod config;
mod db;
mod file_dialog;
mod jira;
mod link_title;
mod mcp;
mod model;
mod service;
mod web;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

const USAGE: &str = "\
使い方: my_todo [serve] [オプション]

サブコマンド:
  serve            ローカル Web アプリを起動する（既定）
  mcp              MCP サーバーとして標準入出力で待ち受ける（AI から使う）

オプション:
  --db <PATH>      データベースファイル（既定: 前回開いたファイル。初めてなら実行ファイルと
                   同じ場所の my_todo.db。target/ 配下から起動したときはカレントディレクトリ）
  --port <PORT>    待ち受けポート（既定: 8765。127.0.0.1 のみ）
  --no_open        起動時にブラウザを開かない
  --keep_running   画面を閉じても終了しない（既定は、閉じて 1 分ほどで終了する）
  -h, --help       この説明を表示する";

#[derive(PartialEq)]
enum Mode {
    Serve,
    Mcp,
}

struct Options {
    mode: Mode,
    /// --db で指定したとき
    db_path: Option<PathBuf>,
    port: u16,
    open_browser: bool,
    keep_running: bool,
}

/// 既定は実行ファイルと同じ場所の my_todo.db。
/// ただし cargo run などで target/debug・target/release から起動したときは、
/// cargo clean でデータが消えないようにカレントディレクトリに置く。
fn default_db_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    match exe_dir {
        Some(dir) if !is_cargo_target_dir(&dir) => dir.join("my_todo.db"),
        _ => PathBuf::from("my_todo.db"),
    }
}

fn is_cargo_target_dir(dir: &std::path::Path) -> bool {
    dir.ancestors()
        .any(|d| d.file_name().is_some_and(|n| n == "target"))
}

fn parse_args() -> Result<Option<Options>> {
    let mut opts = Options {
        mode: Mode::Serve,
        db_path: None,
        port: 8765,
        open_browser: true,
        keep_running: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "serve" => opts.mode = Mode::Serve,
            "mcp" => opts.mode = Mode::Mcp,
            "--db" => opts.db_path = Some(args.next().context("--db に値がありません")?.into()),
            "--port" => {
                opts.port = args
                    .next()
                    .context("--port に値がありません")?
                    .parse()
                    .context("--port は数値で指定してください")?
            }
            "--no_open" => opts.open_browser = false,
            "--keep_running" => opts.keep_running = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => bail!("不明な引数です: {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(opts))
}

/// 開くデータベース：--db の指定 → 前回開いたファイル → 実行ファイルと同じ場所
fn resolve_db_path(opts: &Options) -> PathBuf {
    opts.db_path
        .clone()
        .or_else(|| config::Config::load().last_db())
        .unwrap_or_else(default_db_path)
}

fn main() {
    #[cfg(windows)]
    win::attach_parent_console();
    // 窓がないので、アプリ自体が終わってしまう異常（main スレッドの panic）はダイアログで知らせる。
    // 要求ごとの処理（tokio のワーカー）の panic はアプリが続くので出さない
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        if std::thread::current().name() == Some("main") {
            show_error(&format!("my_todo が異常終了しました。\n\n{info}"));
        }
    }));
    let result = tokio::runtime::Runtime::new()
        .context("非同期処理の準備ができません")
        .and_then(|rt| rt.block_on(run()));
    if let Err(e) = result {
        eprintln!("my_todo: エラー: {e:#}");
        show_error(&format!("my_todo を起動できませんでした。\n\n{e:#}"));
        std::process::exit(1);
    }
}

/// エラーをダイアログで出す（Windows のみ）。AI（MCP）から起動されたときは、
/// 相手のプロセスを止めないよう出さずに標準エラーだけにする
fn show_error(msg: &str) {
    if std::env::args().any(|a| a == "mcp") {
        return;
    }
    #[cfg(windows)]
    win::message_box(msg);
    #[cfg(not(windows))]
    let _ = msg;
}

async fn run() -> Result<()> {
    let Some(opts) = parse_args()? else {
        return Ok(());
    };
    let db_path = resolve_db_path(&opts);
    let conn = db::open(&db_path)?;
    config::remember_db(&db_path);
    match opts.mode {
        // 標準出力は JSON-RPC 専用なので、案内は標準エラーへ
        Mode::Mcp => {
            eprintln!("my_todo mcp: データベース {}", db_path.display());
            tokio::task::block_in_place(|| mcp::run(&conn))
        }
        Mode::Serve => {
            println!("my_todo: データベース {}", db_path.display());
            web::serve(
                conn,
                db_path,
                opts.port,
                opts.open_browser,
                opts.keep_running,
            )
            .await
        }
    }
}

/// Windows の API を呼ぶところ（ここだけ unsafe を許す）
#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    /// 窓のないアプリなので、コマンドプロンプトから起動したときはその窓につないで文字（--help など）を出す。
    /// AI（MCP）のように標準入出力を渡されたときは、つながずにそのまま使う
    pub fn attach_parent_console() {
        // SAFETY: 引数は定数だけ。戻り値のハンドルは比べるだけで使わない
        let out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            // SAFETY: 引数は定数だけ。失敗しても（親に窓がない）何もしないだけ
            unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        }
    }

    pub fn message_box(msg: &str) {
        let text: Vec<u16> = msg.encode_utf16().chain([0]).collect();
        let title: Vec<u16> = "my_todo".encode_utf16().chain([0]).collect();
        // SAFETY: どちらも 0 で終わる UTF-16 の文字列で、呼んでいる間は生きている
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
    }
}
