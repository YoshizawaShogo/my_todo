#[macro_use]
mod str_enum;

mod config;
mod db;
mod jira;
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
    // ダブルクリックで起動したとき、エラーや異常終了のメッセージが読めるように窓を閉じずに待つ
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        // 要求ごとの処理（tokio のワーカー）の panic はアプリが続くので待たない。
        // 待つのはアプリ自体が終わる main スレッドの panic だけ
        if std::thread::current().name() == Some("main") {
            pause_if_own_console();
        }
    }));
    let result = tokio::runtime::Runtime::new()
        .context("非同期処理の準備ができません")
        .and_then(|rt| rt.block_on(run()));
    if let Err(e) = result {
        eprintln!("my_todo: エラー: {e:#}");
        pause_if_own_console();
        std::process::exit(1);
    }
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
            web::serve(conn, db_path, opts.port, opts.open_browser).await
        }
    }
}

/// この窓（コンソール）を使っているのが自分だけ＝ダブルクリックで起動したときは、
/// 閉じる前に Enter を待つ。コマンドプロンプトなどから起動したときは待たない
#[cfg(windows)]
#[allow(unsafe_code)] // Windows の API（GetConsoleProcessList）を呼ぶため。引数は長さを合わせた配列だけ
fn pause_if_own_console() {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    let mut pids = [0u32; 2];
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if count == 1 {
        eprintln!("\nEnter キーを押すと閉じます。");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

#[cfg(not(windows))]
fn pause_if_own_console() {}
