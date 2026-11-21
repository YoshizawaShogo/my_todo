//! ユーザーごとの設定ファイル。最近開いたデータベースの一覧を覚える。
//!
//! 場所：Windows は %APPDATA%\my_todo\config.json、それ以外は
//! $XDG_CONFIG_HOME/my_todo/config.json（なければ ~/.config/my_todo/config.json）。
//! データベースの中身とは別に持つ（どの DB を開いたかは DB の外の情報なので）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最近開いたデータベースを覚えておく数
const MAX_RECENT: usize = 10;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// 最近開いたデータベース（新しい順）
    #[serde(default)]
    pub recent_dbs: Vec<PathBuf>,
}

fn config_path() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(dir.join("my_todo").join("config.json"))
}

impl Config {
    /// 読めなければ空の設定（設定ファイルが壊れていても起動はできるように）
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// 前回開いたデータベース（まだ存在するもの）
    pub fn last_db(&self) -> Option<PathBuf> {
        self.recent_dbs.iter().find(|p| p.is_file()).cloned()
    }

    /// 開いたデータベースを一覧の先頭に入れる
    pub fn remember(&mut self, path: &Path) {
        let path = normalize(path);
        self.recent_dbs.retain(|p| normalize(p) != path);
        self.recent_dbs.insert(0, path);
        self.recent_dbs.truncate(MAX_RECENT);
    }

    pub fn forget(&mut self, path: &Path) {
        let path = normalize(path);
        self.recent_dbs.retain(|p| normalize(p) != path);
    }
}

/// 比べやすい形にする（絶対パスにし、Windows の \\?\ を外す）
pub fn normalize(path: &Path) -> PathBuf {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    });
    let s = abs.display().to_string();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

/// 開いたことを設定ファイルに記録する（失敗しても動作は続ける）
pub fn remember_db(path: &Path) {
    let mut c = Config::load();
    c.remember(path);
    if let Err(e) = c.save() {
        eprintln!("my_todo: 最近開いたファイルの一覧を保存できません: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_moves_to_front_without_duplicates_and_caps() {
        let mut c = Config::default();
        for i in 0..12 {
            c.remember(Path::new(&format!("/tmp/my_todo_cfg_test/{i}.db")));
        }
        assert_eq!(c.recent_dbs.len(), MAX_RECENT);
        assert_eq!(
            c.recent_dbs[0],
            PathBuf::from("/tmp/my_todo_cfg_test/11.db")
        );
        c.remember(Path::new("/tmp/my_todo_cfg_test/5.db"));
        assert_eq!(c.recent_dbs[0], PathBuf::from("/tmp/my_todo_cfg_test/5.db"));
        assert_eq!(
            c.recent_dbs.iter().filter(|p| p.ends_with("5.db")).count(),
            1
        );
        c.forget(Path::new("/tmp/my_todo_cfg_test/5.db"));
        assert!(!c.recent_dbs.iter().any(|p| p.ends_with("5.db")));
        // 存在しないものは「前回の DB」に選ばない
        assert_eq!(c.last_db(), None);
    }

    #[test]
    fn broken_or_missing_config_is_empty() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.recent_dbs.is_empty());
    }
}
