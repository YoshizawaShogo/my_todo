use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, TransactionBehavior};

/// 番号順に適用するマイグレーション。PRAGMA user_version に適用済みの数を記録する。
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_schedule_and_history.sql"),
    include_str!("../migrations/003_tags.sql"),
    include_str!("../migrations/004_undo.sql"),
    include_str!("../migrations/005_time_tracking.sql"),
    include_str!("../migrations/006_jira.sql"),
    include_str!("../migrations/007_templates.sql"),
    include_str!("../migrations/008_template_workdays.sql"),
    include_str!("../migrations/009_task_links.sql"),
];

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("データベースを開けません: {}", path.display()))?;
    // 画面（serve）と MCP（mcp）が別プロセスで同じファイルを開くので、書き込みが重なったら待つ
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // ほかのアプリの SQLite ファイルを選んでしまったときに、表を足したり WAL に変えたりしない
    check_is_my_todo(&conn).with_context(|| format!("{}", path.display()))?;
    set_wal(&conn)?;
    prepare(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    prepare(Connection::open_in_memory()?)
}

fn prepare(conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    // 前回の実行が途中で止まり、元に戻すの記録モードのまま残っていたら戻す
    conn.execute(
        "UPDATE undo_state SET mode = NULL, step = NULL WHERE id = 1",
        [],
    )?;
    Ok(conn)
}

/// WAL に切り替える。切り替えは busy_timeout で待ってくれないので、ほかのプロセスが同時に開いていて
/// ロックが取れないときは少し待ってやり直す
fn set_wal(conn: &Connection) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(e).context("WAL に切り替えられません"),
        }
    }
}

fn user_version(conn: &Connection) -> Result<usize> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(usize::try_from(v).unwrap_or(usize::MAX))
}

/// 空のファイルか、my_todo のデータベースか。新しい版の my_todo で作られたものも開かない
fn check_is_my_todo(conn: &Connection) -> Result<()> {
    // 版と表の一覧を同じ時点で読む（別々に読むと、間にほかのプロセスがマイグレーションを確定して食い違う）
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let conn = &*tx;
    let version = user_version(conn)?;
    if version > MIGRATIONS.len() {
        anyhow::bail!(
            "このデータベースは新しい版の my_todo で作られています（形式 {version}、この版は {} まで）。新しい版の my_todo で開いてください",
            MIGRATIONS.len()
        );
    }
    let has_tables: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%')",
        [],
        |r| r.get(0),
    )?;
    let has_task: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task')",
        [],
        |r| r.get(0),
    )?;
    if has_tables && (version == 0 || !has_task) {
        anyhow::bail!(
            "my_todo のデータベースではありません（ほかのアプリのファイルかもしれません）"
        );
    }
    tx.commit()?;
    Ok(())
}

/// 未適用のマイグレーションを 1 つずつ適用する。画面と MCP が同時に起動しても二重に適用しないよう、
/// 書き込みのロックを取ってから（BEGIN IMMEDIATE）適用済みの数を読み直す
fn migrate(conn: &Connection) -> Result<()> {
    loop {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let applied = user_version(&tx)?;
        let Some(sql) = MIGRATIONS.get(applied) else {
            return Ok(());
        };
        tx.execute_batch(sql)
            .with_context(|| format!("マイグレーション {} に失敗しました", applied + 1))?;
        tx.pragma_update(None, "user_version", (applied + 1) as i64)?;
        tx.commit()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("my_todo_db_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn refuses_other_apps_and_newer_files() {
        let d = dir();
        let other = d.join("other.db");
        let _ = std::fs::remove_file(&other);
        Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY)")
            .unwrap();
        assert!(open(&other).is_err());
        // WAL にも変えていない
        let mode: String = Connection::open(&other)
            .unwrap()
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete");

        let newer = d.join("newer.db");
        let _ = std::fs::remove_file(&newer);
        drop(open(&newer).unwrap());
        Connection::open(&newer)
            .unwrap()
            .pragma_update(None, "user_version", 999)
            .unwrap();
        assert!(open(&newer).is_err());
    }

    /// 画面と MCP が同時に新しいファイルを開いても、どちらも失敗しない
    #[test]
    fn concurrent_first_open_succeeds() {
        let d = dir();
        for round in 0..10 {
            let path = d.join(format!("race{round}.db"));
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
            }
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let p = path.clone();
                    std::thread::spawn(move || open(&p).map(|_| ()).map_err(|e| format!("{e:#}")))
                })
                .collect();
            for h in handles {
                h.join().unwrap().unwrap();
            }
        }
    }
}
