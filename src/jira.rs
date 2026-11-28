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
/// 前の版で使っていた、URL に結び付いていない保存先（見つけたら消す）
const KEYRING_USER_LEGACY: &str = "jira_pat";
const TOKEN_ENV: &str = "MY_TODO_JIRA_TOKEN";

// ---------- PAT の保管 ----------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Env,
    Keyring,
}

/// OS の資格情報での保存名。PAT は Jira の URL（scheme://host:port）ごとに分けて保存する。
/// データベースごとに URL を持つので、人からもらったデータベースを開いても、
/// 自分の PAT がそのデータベースに書かれた別の URL へ送られないようにするため
fn keyring_user(base_url: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(base_url)
        .map_err(|e| format!("Jira の URL が正しくありません: {e}"))?;
    let host = u.host_str().ok_or("Jira の URL にホスト名がありません")?;
    let port = u.port_or_known_default().unwrap_or(0);
    Ok(format!(
        "jira_pat:{}://{}:{port}",
        u.scheme(),
        host.to_lowercase()
    ))
}

/// その URL に使う PAT。環境変数があればそれを（URL を問わず）使う
pub fn load_token(base_url: Option<&str>) -> Option<(String, TokenSource)> {
    if let Ok(t) = std::env::var(TOKEN_ENV)
        && !t.trim().is_empty()
    {
        return Some((t.trim().to_string(), TokenSource::Env));
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_user(base_url?).ok()?).ok()?;
    entry
        .get_password()
        .ok()
        .filter(|t| !t.is_empty())
        .map(|t| (t, TokenSource::Keyring))
}

/// その URL 用の PAT を保存する（空文字なら削除）
pub fn save_token(base_url: Option<&str>, token: &str) -> Result<(), String> {
    let base_url =
        base_url.ok_or("先に Jira の URL を保存してください（PAT は URL ごとに保存します）")?;
    // 前の版の、URL に結び付いていない保存先は消しておく
    if let Ok(old) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER_LEGACY) {
        let _ = old.delete_credential();
    }
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, &keyring_user(base_url)?).map_err(keyring_error)?;
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
    /// 担当者のユーザー名（いなければ None）。自分の課題かどうかの判定に使う
    #[serde(skip)]
    pub assignee: Option<String>,
}

/// 同期する項目の値（日付は YYYY-MM-DD、見積もりは分）
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IssueFields {
    pub key: String,
    #[serde(skip)]
    pub assignee: Option<String>,
    /// 期日（duedate）
    pub due: Option<String>,
    /// 開始日（会社ごとの独自の項目。設定していなければ None）
    pub start: Option<String>,
    /// 初期見積もり（分）
    pub estimate_min: Option<i64>,
}

/// 課題に書き込む項目。None の項目は変えない（Some(None) は空にする）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldUpdate {
    pub due: Option<Option<String>>,
    pub start: Option<Option<String>>,
    pub estimate_min: Option<i64>,
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
    /// 期日・開始日・初期見積もりを読む。start_field は開始日に使う項目（なければ開始日は読まない）
    async fn issue_fields(
        &self,
        key: &str,
        start_field: Option<&str>,
    ) -> Result<IssueFields, String>;
    /// 期日・開始日・初期見積もりを書き込む
    async fn update_fields(
        &self,
        key: &str,
        update: &FieldUpdate,
        start_field: Option<&str>,
    ) -> Result<(), String>;
    /// このアプリが作ったのと同じ作業記録（author が自分・その日の 9:00 開始・同じ時間）がすでにあれば、その id。
    /// 作成の応答が届かなかった（通信が切れた・時間切れ）あとに送り直して、二重に作らないため
    async fn find_worklog(
        &self,
        key: &str,
        author: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<Option<String>, String>;
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

// ---------- 自分の課題だけ ----------
/// Jira の権限に関わらず、担当者が自分の課題だけを読み書きする（アプリ側の制限）。
/// すべての Jira への要求はこれを通す。担当者が自分でない課題は、読むのも書くのもここで断る。
/// 新しい課題を作るのは制限しない（作る機能を足すときも、担当者を自分にして作る）
pub struct MineOnly<J> {
    inner: J,
    /// 自分のユーザー名（最初に 1 回だけ Jira に聞く）
    me: std::sync::Mutex<Option<String>>,
    /// 自分の課題と確かめた課題キー（1 回の処理の間だけ覚える）
    checked: std::sync::Mutex<std::collections::HashSet<String>>,
}

pub fn not_mine(key: &str) -> String {
    format!(
        "{key}: 担当者が自分ではない課題なので、このアプリからは読み書きしません（自分の課題だけを扱う設定）"
    )
}

impl<J: JiraApi> MineOnly<J> {
    pub fn new(inner: J) -> Self {
        Self {
            inner,
            me: std::sync::Mutex::new(None),
            checked: std::sync::Mutex::new(Default::default()),
        }
    }

    async fn me(&self) -> Result<String, String> {
        if let Some(m) = self.me.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(m);
        }
        let m = self.inner.myself().await?.name;
        if m.is_empty() {
            return Err("自分のユーザー名を Jira から取れませんでした".into());
        }
        *self.me.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.clone());
        Ok(m)
    }

    fn is_me(me: &str, assignee: &Option<String>) -> bool {
        assignee.as_deref() == Some(me)
    }

    /// 担当者が自分の課題か確かめる（違えば断る）
    async fn ensure_mine(&self, key: &str) -> Result<(), String> {
        if self
            .checked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key)
        {
            return Ok(());
        }
        let me = self.me().await?;
        let info = self.inner.issue(key).await?;
        if !Self::is_me(&me, &info.assignee) {
            return Err(not_mine(key));
        }
        self.checked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string());
        Ok(())
    }
}

/// 自分が担当の課題だけに絞る JQL（ORDER BY は後ろに残す）
fn mine_jql(jql: &str) -> String {
    let lower = jql.to_lowercase();
    let (cond, order) = match lower.rfind("order by") {
        Some(i) => (jql[..i].trim(), jql[i..].trim()),
        None => (jql.trim(), ""),
    };
    let base = if cond.is_empty() {
        "assignee = currentUser()".to_string()
    } else {
        format!("assignee = currentUser() AND ({cond})")
    };
    format!("{base} {order}").trim().to_string()
}

impl<J: JiraApi> JiraApi for MineOnly<J> {
    async fn myself(&self) -> Result<Myself, String> {
        self.inner.myself().await
    }
    async fn issue(&self, key: &str) -> Result<IssueInfo, String> {
        let me = self.me().await?;
        let info = self.inner.issue(key).await?;
        if !Self::is_me(&me, &info.assignee) {
            return Err(not_mine(key));
        }
        Ok(info)
    }
    async fn search(&self, jql: &str, max: usize) -> Result<Vec<IssueInfo>, String> {
        let me = self.me().await?;
        // JQL で絞ったうえで、返ってきたものも担当者で確かめる（念のため二重に）
        let list = self.inner.search(&mine_jql(jql), max).await?;
        Ok(list
            .into_iter()
            .filter(|i| Self::is_me(&me, &i.assignee))
            .collect())
    }
    async fn add_worklog(
        &self,
        key: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<String, String> {
        self.ensure_mine(key).await?;
        self.inner.add_worklog(key, work_date, minutes).await
    }
    async fn update_worklog(
        &self,
        key: &str,
        id: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<(), String> {
        self.ensure_mine(key).await?;
        self.inner.update_worklog(key, id, work_date, minutes).await
    }
    async fn delete_worklog(&self, key: &str, id: &str) -> Result<(), String> {
        self.ensure_mine(key).await?;
        self.inner.delete_worklog(key, id).await
    }
    async fn issue_fields(
        &self,
        key: &str,
        start_field: Option<&str>,
    ) -> Result<IssueFields, String> {
        let me = self.me().await?;
        let f = self.inner.issue_fields(key, start_field).await?;
        if !Self::is_me(&me, &f.assignee) {
            return Err(not_mine(key));
        }
        Ok(f)
    }
    async fn update_fields(
        &self,
        key: &str,
        update: &FieldUpdate,
        start_field: Option<&str>,
    ) -> Result<(), String> {
        self.ensure_mine(key).await?;
        self.inner.update_fields(key, update, start_field).await
    }
    async fn find_worklog(
        &self,
        key: &str,
        author: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<Option<String>, String> {
        self.ensure_mine(key).await?;
        self.inner
            .find_worklog(key, author, work_date, minutes)
            .await
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
                    .get(self.url(&format!("issue/{key}?fields=summary,status,assignee"))),
                key,
            )
            .await?;
        Ok(IssueInfo {
            key: v["key"].as_str().unwrap_or(key).into(),
            summary: v["fields"]["summary"].as_str().unwrap_or("").into(),
            status: v["fields"]["status"]["name"].as_str().unwrap_or("").into(),
            assignee: v["fields"]["assignee"]["name"].as_str().map(String::from),
        })
    }

    async fn issue_fields(
        &self,
        key: &str,
        start_field: Option<&str>,
    ) -> Result<IssueFields, String> {
        let mut fields = "assignee,duedate,timeoriginalestimate".to_string();
        if let Some(f) = start_field {
            fields.push(',');
            fields.push_str(f);
        }
        let v = self
            .send(
                self.http
                    .get(self.url(&format!("issue/{key}")))
                    .query(&[("fields", fields.as_str())]),
                key,
            )
            .await?;
        let f = &v["fields"];
        let date = |x: &Value| x.as_str().and_then(|s| s.get(..10)).map(String::from);
        Ok(IssueFields {
            key: v["key"].as_str().unwrap_or(key).into(),
            assignee: f["assignee"]["name"].as_str().map(String::from),
            due: date(&f["duedate"]),
            start: start_field.and_then(|sf| date(&f[sf])),
            estimate_min: f["timeoriginalestimate"].as_i64().map(|s| s / 60),
        })
    }

    async fn update_fields(
        &self,
        key: &str,
        update: &FieldUpdate,
        start_field: Option<&str>,
    ) -> Result<(), String> {
        let mut fields = serde_json::Map::new();
        if let Some(due) = &update.due {
            fields.insert("duedate".into(), json!(due));
        }
        if let (Some(start), Some(sf)) = (&update.start, start_field) {
            fields.insert(sf.into(), json!(start));
        }
        if let Some(m) = update.estimate_min {
            fields.insert(
                "timetracking".into(),
                json!({ "originalEstimate": format!("{m}m") }),
            );
        }
        if fields.is_empty() {
            return Ok(());
        }
        self.send(
            self.http
                .put(self.url(&format!("issue/{key}")))
                .json(&json!({ "fields": fields })),
            key,
        )
        .await
        .map(|_| ())
    }

    async fn search(&self, jql: &str, max: usize) -> Result<Vec<IssueInfo>, String> {
        let max = max.to_string();
        let req = self.http.get(self.url("search")).query(&[
            ("jql", jql),
            ("fields", "summary,status,assignee"),
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
                        assignee: i["fields"]["assignee"]["name"].as_str().map(String::from),
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

    async fn find_worklog(
        &self,
        key: &str,
        author: &str,
        work_date: &str,
        minutes: i64,
    ) -> Result<Option<String>, String> {
        let v = self
            .send(
                self.http.get(self.url(&format!("issue/{key}/worklog"))),
                key,
            )
            .await?;
        let start = format!("{work_date}T09:00:00");
        Ok(v["worklogs"].as_array().and_then(|list| {
            list.iter()
                .find(|w| {
                    w["author"]["name"].as_str() == Some(author)
                        && w["started"].as_str().is_some_and(|s| s.starts_with(&start))
                        && w["timeSpentSeconds"].as_i64() == Some(minutes * 60)
                })
                .and_then(|w| w["id"].as_str().map(String::from))
        }))
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
/// 画面で確認した 1 行（送る直前に、作り直した下書きと食い違っていないか確かめる）
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ExpectedLine {
    pub work_date: String,
    pub issue_key: String,
    pub action: DraftAction,
    pub minutes: i64,
}

pub async fn send_draft<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
    from: &str,
    to: &str,
    expected: &[ExpectedLine],
) -> Result<Vec<SendResult>, String> {
    let draft = {
        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
        jira_service::build_draft(&c, from, to).map_err(|e| e.to_string())?
    };
    // 確認したあとに（AI などで）工数が変わっていたら送らない。確認していない内容を送らないため
    let pending: Vec<ExpectedLine> = draft
        .lines
        .iter()
        .filter(|l| l.action != DraftAction::Unchanged)
        .map(|l| ExpectedLine {
            work_date: l.work_date.clone(),
            issue_key: l.issue_key.clone(),
            action: l.action,
            minutes: l.minutes,
        })
        .collect();
    if pending != expected {
        return Err(CHANGED_SINCE_CONFIRM.into());
    }
    // 作り直しの確認に使う自分のユーザー名（作成するものがあるときだけ取る）
    let me = if pending.iter().any(|l| l.action == DraftAction::Create) {
        Some(api.myself().await?.name)
    } else {
        None
    };
    let mut results = Vec::new();
    for line in draft
        .lines
        .into_iter()
        .filter(|l| l.action != DraftAction::Unchanged)
    {
        let outcome: Result<(), String> = match line.action {
            DraftAction::Create => match create_once(api, me.as_deref(), &line).await {
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

// ---------- 期日・開始日・見積もりの同期 ----------
/// 同期する項目
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncField {
    /// 期日 ↔ 終了予定日
    Due,
    /// 開始日 ↔ 開始予定日
    Start,
    /// 初期見積もり ↔ 見積もり
    Estimate,
}

/// アプリと Jira で値が違う 1 項目（日付は YYYY-MM-DD、見積もりは分を文字にしたもの）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct FieldDiff {
    pub task_id: i64,
    pub title: String,
    pub issue_key: String,
    pub field: SyncField,
    pub app: Option<String>,
    pub jira: Option<String>,
}

/// 比べられなかった課題（担当者が自分でない・見つからないなど）
#[derive(Debug, Clone, Serialize)]
pub struct FieldProblem {
    pub issue_key: String,
    pub title: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct FieldCompare {
    pub diffs: Vec<FieldDiff>,
    pub problems: Vec<FieldProblem>,
}

/// 課題キーを付けた自分のタスクについて、期日・開始日・見積もりをアプリと Jira で比べる
pub async fn compare_fields<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
) -> Result<FieldCompare, String> {
    let (tasks, start_field) = {
        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
        let tasks = crate::service::task_service::list_tasks(&c).map_err(|e| e.to_string())?;
        let start = jira_service::get_config(&c)
            .map_err(|e| e.to_string())?
            .start_field;
        (tasks, start)
    };
    let mut by_key: std::collections::BTreeMap<String, Vec<&crate::model::Task>> =
        Default::default();
    for t in tasks
        .iter()
        .filter(|t| t.kind == crate::model::TaskKind::Own)
    {
        if let Some(k) = &t.jira_key {
            by_key.entry(k.clone()).or_default().push(t);
        }
    }
    let mut out = FieldCompare {
        diffs: Vec::new(),
        problems: Vec::new(),
    };
    for (key, ts) in by_key {
        let t = ts[0];
        if ts.len() > 1 {
            out.problems.push(FieldProblem {
                issue_key: key,
                title: ts
                    .iter()
                    .map(|t| t.title.as_str())
                    .collect::<Vec<_>>()
                    .join("、"),
                error: "同じ課題が複数のタスクに紐付いているので、日程・見積もりは同期しません"
                    .into(),
            });
            continue;
        }
        let f = match api.issue_fields(&key, start_field.as_deref()).await {
            Ok(f) => f,
            Err(e) => {
                out.problems.push(FieldProblem {
                    issue_key: key,
                    title: t.title.clone(),
                    error: e,
                });
                continue;
            }
        };
        let mut push = |field: SyncField, app: Option<String>, jira: Option<String>| {
            if app != jira {
                out.diffs.push(FieldDiff {
                    task_id: t.id,
                    title: t.title.clone(),
                    issue_key: key.clone(),
                    field,
                    app,
                    jira,
                });
            }
        };
        push(SyncField::Due, t.planned_end.clone(), f.due.clone());
        if start_field.is_some() {
            push(SyncField::Start, t.planned_start.clone(), f.start.clone());
        }
        push(
            SyncField::Estimate,
            t.estimate_min.map(|m| m.to_string()),
            f.estimate_min.map(|m| m.to_string()),
        );
    }
    Ok(out)
}

/// 確認した内容（diffs）と今の比べ直しが食い違っていないか
async fn recheck<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
    expected: &[FieldDiff],
) -> Result<FieldCompare, String> {
    let now = compare_fields(api, conn).await?;
    if expected.iter().any(|e| !now.diffs.contains(e)) {
        return Err(CHANGED_SINCE_CONFIRM.into());
    }
    Ok(now)
}

/// アプリの値を Jira に送る（アプリがマスター）。送れなかったものを返す
pub async fn push_fields<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
    expected: &[FieldDiff],
) -> Result<Vec<FieldProblem>, String> {
    recheck(api, conn, expected).await?;
    let start_field = {
        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
        jira_service::get_config(&c)
            .map_err(|e| e.to_string())?
            .start_field
    };
    let mut problems = Vec::new();
    // 課題ごとにまとめて 1 回で書き込む
    let mut by_key: std::collections::BTreeMap<&str, (FieldUpdate, &str)> = Default::default();
    for d in expected {
        let entry = by_key
            .entry(d.issue_key.as_str())
            .or_insert((FieldUpdate::default(), d.title.as_str()));
        match d.field {
            SyncField::Due => entry.0.due = Some(d.app.clone()),
            SyncField::Start => entry.0.start = Some(d.app.clone()),
            SyncField::Estimate => match d.app.as_deref().and_then(|m| m.parse().ok()) {
                Some(m) => entry.0.estimate_min = Some(m),
                None => problems.push(FieldProblem {
                    issue_key: d.issue_key.clone(),
                    title: d.title.clone(),
                    error: "見積もりを空にするのは Jira の画面で行ってください".into(),
                }),
            },
        }
    }
    for (key, (update, title)) in by_key {
        if let Err(e) = api
            .update_fields(key, &update, start_field.as_deref())
            .await
        {
            problems.push(FieldProblem {
                issue_key: key.into(),
                title: title.into(),
                error: e,
            });
        }
    }
    Ok(problems)
}

/// Jira の値をアプリに取り込む（Jira がマスター）。1 回の操作として元に戻せる。取り込めなかったものを返す
pub async fn pull_fields<J: JiraApi>(
    api: &J,
    conn: &Mutex<Connection>,
    expected: &[FieldDiff],
) -> Result<Vec<FieldProblem>, String> {
    recheck(api, conn, expected).await?;
    let c = conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut problems = Vec::new();
    let mut by_task: std::collections::BTreeMap<i64, Vec<&FieldDiff>> = Default::default();
    for d in expected {
        by_task.entry(d.task_id).or_default().push(d);
    }
    crate::service::undo_service::record(
        &c,
        "Jira から日程・見積もりを取り込み",
        |c| {
            for (task_id, ds) in &by_task {
                let mut patch = crate::model::TaskPatch::default();
                for d in ds {
                    match d.field {
                        SyncField::Due => patch.planned_end = Some(d.jira.clone()),
                        SyncField::Start => patch.planned_start = Some(d.jira.clone()),
                        SyncField::Estimate => {
                            patch.estimate_min =
                                Some(d.jira.as_deref().and_then(|m| m.parse().ok()))
                        }
                    }
                }
                // 1 件ずつ。完了したタスクなどで取り込めないものは飛ばして知らせる
                if let Err(e) = crate::service::task_service::update_task(c, *task_id, patch) {
                    problems.push(FieldProblem {
                        issue_key: ds[0].issue_key.clone(),
                        title: ds[0].title.clone(),
                        error: e.to_string(),
                    });
                }
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(problems)
}

pub const CHANGED_SINCE_CONFIRM: &str = "確認したあとに送る内容が変わりました（AI などで工数が変わった可能性があります）。内容を確認し直してから送ってください";

/// 作業記録を作る。前回の送信で作れていたのに応答が届かなかったもの（同じ記録がすでにある）は、作らずにその id を使う
async fn create_once<J: JiraApi>(
    api: &J,
    me: Option<&str>,
    line: &jira_service::DraftLine,
) -> Result<String, String> {
    if let Some(me) = me
        && let Some(id) = api
            .find_worklog(&line.issue_key, me, &line.work_date, line.minutes)
            .await?
    {
        return Ok(id);
    }
    api.add_worklog(&line.issue_key, &line.work_date, line.minutes)
        .await
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
        /// Jira 側にすでにある作業記録（課題, 日付, 分, id）。作成の応答が届かなかった状況を表す
        existing: Vec<(String, String, i64, String)>,
        /// 担当者が自分ではない課題
        others: Vec<String>,
        /// 課題ごとの期日・開始日・見積もり
        fields: RefCell<std::collections::HashMap<String, IssueFields>>,
        /// search に渡された JQL
        last_jql: RefCell<String>,
    }

    impl FakeJira {
        fn assignee(&self, key: &str) -> Option<String> {
            Some(if self.others.iter().any(|k| k == key) {
                "other".into()
            } else {
                "me".into()
            })
        }
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
                assignee: self.assignee(key),
            })
        }
        async fn search(&self, jql: &str, _max: usize) -> Result<Vec<IssueInfo>, String> {
            *self.last_jql.borrow_mut() = jql.into();
            // 本物の Jira が JQL を無視して他人の課題も返したとしても、制限で落ちることを確かめる
            Ok(["ABC-1", "XYZ-9"]
                .iter()
                .map(|k| IssueInfo {
                    key: (*k).into(),
                    summary: "s".into(),
                    status: "Open".into(),
                    assignee: self.assignee(k),
                })
                .collect())
        }
        async fn issue_fields(&self, key: &str, _sf: Option<&str>) -> Result<IssueFields, String> {
            let mut f = self.fields.borrow().get(key).cloned().unwrap_or_default();
            f.key = key.into();
            f.assignee = self.assignee(key);
            Ok(f)
        }
        async fn update_fields(
            &self,
            key: &str,
            u: &FieldUpdate,
            _sf: Option<&str>,
        ) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("fields {key}"));
            let mut map = self.fields.borrow_mut();
            let f = map.entry(key.into()).or_default();
            if let Some(d) = &u.due {
                f.due = d.clone();
            }
            if let Some(d) = &u.start {
                f.start = d.clone();
            }
            if let Some(m) = u.estimate_min {
                f.estimate_min = Some(m);
            }
            Ok(())
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
        async fn find_worklog(
            &self,
            key: &str,
            author: &str,
            d: &str,
            m: i64,
        ) -> Result<Option<String>, String> {
            assert_eq!(author, "me");
            Ok(self
                .existing
                .iter()
                .find(|(k, date, min, _)| k == key && date == d && *min == m)
                .map(|(_, _, _, id)| id.clone()))
        }
    }

    /// 画面と同じく、いまの下書きを確認した内容として送る
    fn send(jira: &FakeJira, conn: &Mutex<Connection>) -> Result<Vec<SendResult>, String> {
        let expected: Vec<ExpectedLine> = {
            let c = conn.lock().unwrap();
            jira_service::build_draft(&c, "2026-11-16", "2026-11-22")
                .unwrap()
                .lines
                .into_iter()
                .filter(|l| l.action != DraftAction::Unchanged)
                .map(|l| ExpectedLine {
                    work_date: l.work_date,
                    issue_key: l.issue_key,
                    action: l.action,
                    minutes: l.minutes,
                })
                .collect()
        };
        block(send_draft(
            jira,
            conn,
            "2026-11-16",
            "2026-11-22",
            &expected,
        ))
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
        let r = send(&jira, &conn).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|x| x.error.is_none()));
        // 2 回目は何も送らない
        let r = send(&jira, &conn).unwrap();
        assert!(r.is_empty());
        // B を外すと A は更新、B は削除
        day(&conn, 480, &[(a, 1.0)]);
        send(&jira, &conn).unwrap();
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
        let r = send(&jira, &conn).unwrap();
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

    /// 前回の作成が Jira に届いていたのに応答が来なかった：同じ記録を見つけて使い、二重に作らない
    #[test]
    fn create_reuses_worklog_that_already_exists() {
        let (conn, a, _) = setup();
        day(&conn, 480, &[(a, 1.0)]);
        let jira = FakeJira {
            existing: vec![("ABC-1".into(), "2026-11-20".into(), 480, "w-lost".into())],
            ..Default::default()
        };
        let r = send(&jira, &conn).unwrap();
        assert!(r.iter().all(|x| x.error.is_none()));
        assert!(jira.calls.borrow().is_empty(), "{:?}", jira.calls.borrow());
        let id: String = conn
            .lock()
            .unwrap()
            .query_row("SELECT worklog_id FROM jira_worklog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, "w-lost");
    }

    /// 確認したあとに工数が変わったら、何も送らない
    #[test]
    fn refuses_when_draft_changed_after_confirmation() {
        let (conn, a, b) = setup();
        day(&conn, 480, &[(a, 1.0)]);
        let jira = FakeJira::default();
        let stale = vec![ExpectedLine {
            work_date: "2026-11-20".into(),
            issue_key: "ABC-1".into(),
            action: DraftAction::Create,
            minutes: 480,
        }];
        // 確認のあとに B にも振り分けた
        day(&conn, 480, &[(a, 1.0), (b, 1.0)]);
        let err = block(send_draft(&jira, &conn, "2026-11-16", "2026-11-22", &stale)).unwrap_err();
        assert_eq!(err, CHANGED_SINCE_CONFIRM);
        assert!(jira.calls.borrow().is_empty());
    }

    /// 担当者が自分でない課題には、権限があっても作業記録を送らない
    #[test]
    fn mine_only_refuses_other_peoples_issues() {
        let (conn, a, b) = setup();
        day(&conn, 480, &[(a, 1.0), (b, 1.0)]);
        let jira = MineOnly::new(FakeJira {
            others: vec!["ABC-2".into()],
            ..Default::default()
        });
        let expected: Vec<ExpectedLine> = {
            let c = conn.lock().unwrap();
            jira_service::build_draft(&c, "2026-11-16", "2026-11-22")
                .unwrap()
                .lines
                .into_iter()
                .filter(|l| l.action != DraftAction::Unchanged)
                .map(|l| ExpectedLine {
                    work_date: l.work_date,
                    issue_key: l.issue_key,
                    action: l.action,
                    minutes: l.minutes,
                })
                .collect()
        };
        let r = block(send_draft(
            &jira,
            &conn,
            "2026-11-16",
            "2026-11-22",
            &expected,
        ))
        .unwrap();
        let errs: Vec<_> = r
            .iter()
            .filter(|x| x.error.is_some())
            .map(|x| x.issue_key.as_str())
            .collect();
        assert_eq!(errs, ["ABC-2"]);
        assert_eq!(*jira.inner.calls.borrow(), ["add ABC-1 2026-11-20 240"]);
        // 課題を読むのも断る
        assert!(block(jira.issue("ABC-2")).is_err());
        assert!(block(jira.issue("ABC-1")).is_ok());
    }

    /// 検索は JQL で自分の担当に絞り、返ってきたものも担当者で確かめる
    #[test]
    fn mine_only_search_is_limited() {
        let jira = MineOnly::new(FakeJira {
            others: vec!["XYZ-9".into()],
            ..Default::default()
        });
        let r = block(jira.search("project = ABC ORDER BY updated DESC", 50)).unwrap();
        assert_eq!(
            r.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            ["ABC-1"]
        );
        assert_eq!(
            *jira.inner.last_jql.borrow(),
            "assignee = currentUser() AND (project = ABC) ORDER BY updated DESC"
        );
        assert_eq!(mine_jql(""), "assignee = currentUser()");
    }

    /// 期日・見積もりを比べ、アプリ → Jira に送る・Jira → アプリに取り込む（Ctrl+Z で戻る）
    #[test]
    fn compare_push_and_pull_fields() {
        let (conn, a, b) = setup();
        {
            let c = conn.lock().unwrap();
            let p: TaskPatch =
                serde_json::from_value(json!({ "planned_end": "2026-12-10", "estimate_min": 240 }))
                    .unwrap();
            task_service::update_task(&c, a, p).unwrap();
            let p: TaskPatch =
                serde_json::from_value(json!({ "planned_end": "2026-12-01" })).unwrap();
            task_service::update_task(&c, b, p).unwrap();
        }
        let fake = FakeJira {
            others: vec!["ABC-2".into()],
            ..Default::default()
        };
        fake.fields.borrow_mut().insert(
            "ABC-1".into(),
            IssueFields {
                due: Some("2026-12-05".into()),
                estimate_min: Some(480),
                ..Default::default()
            },
        );
        let jira = MineOnly::new(fake);
        let cmp = block(compare_fields(&jira, &conn)).unwrap();
        let d: Vec<_> = cmp
            .diffs
            .iter()
            .map(|d| (d.issue_key.as_str(), d.field, d.app.clone(), d.jira.clone()))
            .collect();
        assert_eq!(
            d,
            [
                (
                    "ABC-1",
                    SyncField::Due,
                    Some("2026-12-10".into()),
                    Some("2026-12-05".into())
                ),
                (
                    "ABC-1",
                    SyncField::Estimate,
                    Some("240".into()),
                    Some("480".into())
                ),
            ]
        );
        // 他人の課題は比べず、問題として出す
        assert_eq!(
            cmp.problems
                .iter()
                .map(|p| p.issue_key.as_str())
                .collect::<Vec<_>>(),
            ["ABC-2"]
        );

        // 取り込み：アプリが Jira の値になり、Ctrl+Z で戻る
        let probs = block(pull_fields(&jira, &conn, &cmp.diffs)).unwrap();
        assert!(probs.is_empty());
        {
            let c = conn.lock().unwrap();
            let t = task_service::get_task(&c, a).unwrap();
            assert_eq!(
                (t.planned_end.as_deref(), t.estimate_min),
                (Some("2026-12-05"), Some(480))
            );
            crate::service::undo_service::undo(&c).unwrap();
            let t = task_service::get_task(&c, a).unwrap();
            assert_eq!(
                (t.planned_end.as_deref(), t.estimate_min),
                (Some("2026-12-10"), Some(240))
            );
        }
        // 送る：Jira がアプリの値になる
        let cmp = block(compare_fields(&jira, &conn)).unwrap();
        block(push_fields(&jira, &conn, &cmp.diffs)).unwrap();
        assert!(
            block(compare_fields(&jira, &conn))
                .unwrap()
                .diffs
                .is_empty()
        );
        // 確認したあとにアプリが変わっていたら何もしない
        let stale = cmp.diffs.clone();
        {
            let c = conn.lock().unwrap();
            let p: TaskPatch =
                serde_json::from_value(json!({ "planned_end": "2026-12-20" })).unwrap();
            task_service::update_task(&c, a, p).unwrap();
        }
        assert_eq!(
            block(push_fields(&jira, &conn, &stale)).unwrap_err(),
            CHANGED_SINCE_CONFIRM
        );
    }

    #[test]
    fn keyring_user_is_per_url() {
        assert_eq!(
            keyring_user("https://Jira.Example.com/").unwrap(),
            "jira_pat:https://jira.example.com:443"
        );
        assert_eq!(
            keyring_user("http://127.0.0.1:8798").unwrap(),
            "jira_pat:http://127.0.0.1:8798"
        );
        assert_ne!(
            keyring_user("https://jira.example.com").unwrap(),
            keyring_user("https://evil.example.com").unwrap()
        );
    }

    #[test]
    fn started_is_nine_oclock_in_jira_format() {
        let s = started_at("2026-11-20").unwrap();
        assert!(s.starts_with("2026-11-20T09:00:00.000"), "{s}");
        assert!(s.len() == "2026-11-20T09:00:00.000+0900".len(), "{s}");
    }
}
