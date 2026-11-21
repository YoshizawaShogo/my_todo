//! Jira との通信と、PAT（Personal Access Token）の保管。
//!
//! - Cloud 版と社内サーバー版（Data Center / Server）の違いは `JiraApi` トレイトで吸収する。
//!   いまは社内サーバー版（PAT を Bearer で送る、REST API v2）だけを実装している。
//! - PAT はソースにも SQLite にも置かない。環境変数 `MY_TODO_JIRA_TOKEN` があればそれを使い、
//!   なければ OS の資格情報の仕組み（Windows の資格情報マネージャーなど）から読む。
//! - 送信は「下書き → 送信」の 2 段階。送信は差分だけ（新規・更新・削除）を送り、結果を覚える。

use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

use crate::service::jira_service::{self, DraftAction};

const KEYRING_SERVICE: &str = "my_todo";
const KEYRING_USER: &str = "jira_pat";
const TOKEN_ENV: &str = "MY_TODO_JIRA_TOKEN";

// ---------- PAT の保管 ----------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Env,
    Keyring,
}

pub fn load_token() -> Option<(String, TokenSource)> {
    if let Ok(t) = std::env::var(TOKEN_ENV)
        && !t.trim().is_empty()
    {
        return Some((t.trim().to_string(), TokenSource::Env));
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    entry
        .get_password()
        .ok()
        .filter(|t| !t.is_empty())
        .map(|t| (t, TokenSource::Keyring))
}

pub fn save_token(token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(keyring_error)?;
    if token.trim().is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        };
    }
    entry.set_password(token.trim()).map_err(keyring_error)
}

fn keyring_error(e: keyring::Error) -> String {
    format!(
        "PAT を OS の資格情報に保存できません（{e}）。この環境では環境変数 {TOKEN_ENV} に PAT を入れて起動してください"
    )
}

// ---------- 通信 ----------
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Myself {
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueInfo {
    pub key: String,
    pub summary: String,
    pub status: String,
}

/// Jira の版ごとの違いを吸収する。work_date は YYYY-MM-DD（その日の 9:00 に記録する）
#[allow(async_fn_in_trait)] // アプリの中だけで使うので Send の指定は省く
pub trait JiraApi {
    async fn myself(&self) -> Result<Myself, String>;
    async fn issue(&self, key: &str) -> Result<IssueInfo, String>;
    /// JQL で課題を探す（最大 max 件）
    async fn search(&self, jql: &str, max: usize) -> Result<Vec<IssueInfo>, String>;
    /// 作った worklog の id を返す
    async fn add_worklog(&self, key: &str, work_date: &str, minutes: i64)
    -> Result<String, String>;
    async fn update_worklog(
        &self,
        key: &str,
        id: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<(), String>;
    async fn delete_worklog(&self, key: &str, id: &str) -> Result<(), String>;
}

/// 社内サーバー版（Jira Data Center / Server 8.14 以降）。PAT を Bearer で送る
pub struct DataCenterClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl DataCenterClient {
    pub fn new(base_url: &str, token: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("my_todo/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/rest/api/2/{path}", self.base_url)
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<Value, String> {
        let res = req
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("{what}: Jira に接続できません（{e}）"))?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&body).unwrap_or(Value::Null)
            });
        }
        // Jira のエラーは {"errorMessages": [...], "errors": {...}}
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .map(|v| {
                let mut msgs: Vec<String> = v["errorMessages"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| m.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(o) = v["errors"].as_object() {
                    msgs.extend(
                        o.iter()
                            .map(|(k, m)| format!("{k}: {}", m.as_str().unwrap_or(""))),
                    );
                }
                msgs.join(" / ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| body.chars().take(200).collect());
        let hint = match status.as_u16() {
            401 => "PAT が正しくないか、期限が切れています",
            403 => "権限がありません",
            404 => "見つかりません（課題キーや URL を確認してください）",
            _ => "",
        };
        Err(
            format!("{what}: Jira がエラーを返しました（{status}）{hint} {detail}")
                .trim()
                .to_string(),
        )
    }
}

/// worklog の started。その日の 9:00（このPCの時差）を Jira の形式で
fn started_at(work_date: &str) -> Result<String, String> {
    let d = crate::service::parse_date(work_date).map_err(|e| e.to_string())?;
    let dt = d
        .at(9, 0, 0, 0)
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .map_err(|e| e.to_string())?;
    Ok(dt.strftime("%Y-%m-%dT%H:%M:%S%.3f%z").to_string())
}

impl JiraApi for DataCenterClient {
    async fn myself(&self) -> Result<Myself, String> {
        let v = self
            .send(self.http.get(self.url("myself")), "接続テスト")
            .await?;
        Ok(Myself {
            name: v["name"].as_str().unwrap_or("").into(),
            display_name: v["displayName"].as_str().unwrap_or("").into(),
        })
    }

    async fn issue(&self, key: &str) -> Result<IssueInfo, String> {
        let v = self
            .send(
                self.http
                    .get(self.url(&format!("issue/{key}?fields=summary,status"))),
                key,
            )
            .await?;
        Ok(IssueInfo {
            key: v["key"].as_str().unwrap_or(key).into(),
            summary: v["fields"]["summary"].as_str().unwrap_or("").into(),
            status: v["fields"]["status"]["name"].as_str().unwrap_or("").into(),
        })
    }

    async fn search(&self, jql: &str, max: usize) -> Result<Vec<IssueInfo>, String> {
        let max = max.to_string();
        let req = self.http.get(self.url("search")).query(&[
            ("jql", jql),
            ("fields", "summary,status"),
            ("maxResults", max.as_str()),
        ]);
        let v = self.send(req, "課題の検索").await?;
        Ok(v["issues"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| IssueInfo {
                        key: i["key"].as_str().unwrap_or("").into(),
                        summary: i["fields"]["summary"].as_str().unwrap_or("").into(),
                        status: i["fields"]["status"]["name"].as_str().unwrap_or("").into(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn add_worklog(
        &self,
        key: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<String, String> {
        let body = json!({ "started": started_at(work_date)?, "timeSpentSeconds": minutes * 60 });
        let v = self
            .send(
                self.http
                    .post(self.url(&format!("issue/{key}/worklog")))
                    .json(&body),
                key,
            )
            .await?;
        v["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| format!("{key}: worklog の id が返ってきませんでした"))
    }

    async fn update_worklog(
        &self,
        key: &str,
        id: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<(), String> {
        let body = json!({ "started": started_at(work_date)?, "timeSpentSeconds": minutes * 60 });
        self.send(
            self.http
                .put(self.url(&format!("issue/{key}/worklog/{id}")))
                .json(&body),
            key,
        )
        .await
        .map(|_| ())
    }

    async fn delete_worklog(&self, key: &str, id: &str) -> Result<(), String> {
        self.send(
            self.http
                .delete(self.url(&format!("issue/{key}/worklog/{id}"))),
            key,
        )
        .await
        .map(|_| ())
    }
}

// ---------- 送信 ----------
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SendResult {
    pub work_date: String,
    pub issue_key: String,
    pub action: DraftAction,
    pub minutes: i64,
    /// 失敗したときの理由
    pub error: Option<String>,
}

/// 期間の下書きを作り直し、差分を Jira に送る。1 件ずつ送り、成功したものだけ記録する。
/// 通信中は DB のロックを持たない（画面の操作を止めないため）
pub async fn send_draft<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
    from: &str,
    to: &str,
) -> Result<Vec<SendResult>, String> {
    let draft = {
        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
        jira_service::build_draft(&c, from, to).map_err(|e| e.to_string())?
    };
    let mut results = Vec::new();
    for line in draft
        .lines
        .into_iter()
        .filter(|l| l.action != DraftAction::Unchanged)
    {
        let outcome: Result<(), String> = match line.action {
            DraftAction::Create => match api
                .add_worklog(&line.issue_key, &line.work_date, line.minutes)
                .await
            {
                Ok(id) => record(conn, |c| {
                    jira_service::record_sent(
                        c,
                        &line.work_date,
                        &line.issue_key,
                        &id,
                        line.minutes,
                    )
                }),
                Err(e) => Err(e),
            },
            DraftAction::Update => {
                let id = line.worklog_id.clone().unwrap_or_default();
                match api
                    .update_worklog(&line.issue_key, &id, &line.work_date, line.minutes)
                    .await
                {
                    Ok(()) => record(conn, |c| {
                        jira_service::record_sent(
                            c,
                            &line.work_date,
                            &line.issue_key,
                            &id,
                            line.minutes,
                        )
                    }),
                    Err(e) => Err(e),
                }
            }
            DraftAction::Delete => {
                let id = line.worklog_id.clone().unwrap_or_default();
                match api.delete_worklog(&line.issue_key, &id).await {
                    Ok(()) => record(conn, |c| {
                        jira_service::forget_sent(c, &line.work_date, &line.issue_key)
                    }),
                    Err(e) => Err(e),
                }
            }
            DraftAction::Unchanged => Ok(()),
        };
        results.push(SendResult {
            work_date: line.work_date,
            issue_key: line.issue_key,
            action: line.action,
            minutes: line.minutes,
            error: outcome.err(),
        });
    }
    Ok(results)
}

fn record(
    conn: &Mutex<Connection>,
    f: impl FnOnce(&Connection) -> crate::service::ServiceResult<()>,
) -> Result<(), String> {
    let c = conn.lock().unwrap_or_else(|e| e.into_inner());
    f(&c).map_err(|e| format!("送信はできましたが、記録に失敗しました: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::model::{NewTask, TaskPatch};
    use crate::service::task_service;
    use crate::service::work_service::{Allocation, SaveDay, save_day};
    use std::cell::RefCell;

    /// 本物の Jira の代わり。呼ばれた内容を記録し、指定した課題では失敗する
    #[derive(Default)]
    struct FakeJira {
        calls: RefCell<Vec<String>>,
        fail_key: Option<String>,
    }

    impl JiraApi for FakeJira {
        async fn myself(&self) -> Result<Myself, String> {
            Ok(Myself {
                name: "me".into(),
                display_name: "私".into(),
            })
        }
        async fn issue(&self, key: &str) -> Result<IssueInfo, String> {
            Ok(IssueInfo {
                key: key.into(),
                summary: "s".into(),
                status: "Open".into(),
            })
        }
        async fn search(&self, _jql: &str, _max: usize) -> Result<Vec<IssueInfo>, String> {
            Ok(vec![])
        }
        async fn add_worklog(&self, key: &str, d: &str, m: i64) -> Result<String, String> {
            if self.fail_key.as_deref() == Some(key) {
                return Err(format!("{key}: 失敗"));
            }
            self.calls.borrow_mut().push(format!("add {key} {d} {m}"));
            Ok(format!("w-{key}-{d}"))
        }
        async fn update_worklog(&self, key: &str, id: &str, d: &str, m: i64) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("update {key} {id} {d} {m}"));
            Ok(())
        }
        async fn delete_worklog(&self, key: &str, id: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("delete {key} {id}"));
            Ok(())
        }
    }

    fn setup() -> (Mutex<Connection>, i64, i64) {
        let conn = db::open_in_memory().unwrap();
        let mk = |title: &str, key: &str| {
            let t = task_service::create_task(
                &conn,
                NewTask {
                    title: title.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let p: TaskPatch = serde_json::from_value(json!({ "jira_key": key })).unwrap();
            task_service::update_task(&conn, t.id, p).unwrap();
            t.id
        };
        let a = mk("A", "ABC-1");
        let b = mk("B", "ABC-2");
        (Mutex::new(conn), a, b)
    }

    fn day(conn: &Mutex<Connection>, worked: i64, alloc: &[(i64, f64)]) {
        let c = conn.lock().unwrap();
        save_day(
            &c,
            "2026-11-20",
            SaveDay {
                worked_min: worked,
                allocations: alloc
                    .iter()
                    .map(|&(task_id, weight)| Allocation { task_id, weight })
                    .collect(),
            },
        )
        .unwrap();
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn sends_only_differences_and_remembers_them() {
        let (conn, a, b) = setup();
        day(&conn, 480, &[(a, 1.0), (b, 1.0)]);
        let jira = FakeJira::default();
        let r = block(send_draft(&jira, &conn, "2026-11-16", "2026-11-22")).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|x| x.error.is_none()));
        // 2 回目は何も送らない
        let r = block(send_draft(&jira, &conn, "2026-11-16", "2026-11-22")).unwrap();
        assert!(r.is_empty());
        // B を外すと A は更新、B は削除
        day(&conn, 480, &[(a, 1.0)]);
        block(send_draft(&jira, &conn, "2026-11-16", "2026-11-22")).unwrap();
        assert_eq!(
            *jira.calls.borrow(),
            [
                "add ABC-1 2026-11-20 240",
                "add ABC-2 2026-11-20 240",
                "update ABC-1 w-ABC-1-2026-11-20 2026-11-20 480",
                "delete ABC-2 w-ABC-2-2026-11-20",
            ]
        );
    }

    #[test]
    fn failed_lines_are_reported_and_retried_next_time() {
        let (conn, a, b) = setup();
        day(&conn, 480, &[(a, 1.0), (b, 1.0)]);
        let jira = FakeJira {
            fail_key: Some("ABC-2".into()),
            ..Default::default()
        };
        let r = block(send_draft(&jira, &conn, "2026-11-16", "2026-11-22")).unwrap();
        let errors: Vec<_> = r
            .iter()
            .filter(|x| x.error.is_some())
            .map(|x| x.issue_key.as_str())
            .collect();
        assert_eq!(errors, ["ABC-2"]);
        // 失敗した分は未送信のまま残る
        let draft =
            jira_service::build_draft(&conn.lock().unwrap(), "2026-11-16", "2026-11-22").unwrap();
        let actions: Vec<_> = draft
            .lines
            .iter()
            .map(|l| (l.issue_key.as_str(), l.action))
            .collect();
        assert_eq!(
            actions,
            [
                ("ABC-1", DraftAction::Unchanged),
                ("ABC-2", DraftAction::Create)
            ]
        );
    }

    #[test]
    fn started_is_nine_oclock_in_jira_format() {
        let s = started_at("2026-11-20").unwrap();
        assert!(s.starts_with("2026-11-20T09:00:00.000"), "{s}");
        assert!(s.len() == "2026-11-20T09:00:00.000+0900".len(), "{s}");
    }
}
