use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// 番号順に適用するマイグレーション。PRAGMA user_version に適用済みの数を記録する。
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_schedule_and_history.sql"),
    include_str!("../migrations/003_tags.sql"),
    include_str!("../migrations/004_undo.sql"),
    include_str!("../migrations/005_time_tracking.sql"),
];

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("データベースを開けません: {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
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

fn migrate(conn: &Connection) -> Result<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("マイグレーション {} に失敗しました", i + 1))?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
