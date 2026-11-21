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
  --db <PATH>      データベースファイル（既定: 実行ファイルと同じ場所の my_todo.db。
                   target/ 配下から起動したときはカレントディレクトリ）
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
    db_path: PathBuf,
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
        db_path: default_db_path(),
        port: 8765,
        open_browser: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "serve" => opts.mode = Mode::Serve,
            "mcp" => opts.mode = Mode::Mcp,
            "--db" => opts.db_path = args.next().context("--db に値がありません")?.into(),
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

#[tokio::main]
async fn main() -> Result<()> {
    let Some(opts) = parse_args()? else {
        return Ok(());
    };
    let conn = db::open(&opts.db_path)?;
    match opts.mode {
        // 標準出力は JSON-RPC 専用なので、案内は標準エラーへ
        Mode::Mcp => {
            eprintln!("my_todo mcp: データベース {}", opts.db_path.display());
            tokio::task::block_in_place(|| mcp::run(&conn))
        }
        Mode::Serve => {
            println!("my_todo: データベース {}", opts.db_path.display());
            web::serve(conn, opts.db_path, opts.port, opts.open_browser).await
        }
    }
}
