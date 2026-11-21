//! MCP サーバー（`my_todo mcp`）。標準入出力で JSON-RPC 2.0 を 1 行 1 メッセージでやり取りする。
//!
//! 画面と同じサービス層を呼び、変更はすべて undo_service::record で包む。
//! そのため AI が行った変更も、画面の Ctrl+Z で元に戻せる。
//! 標準出力は JSON-RPC 専用。ログは標準エラーへ出す。

use std::collections::HashMap;
use std::io::{BufRead, Write};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::model::{MoveTarget, NewTask, Task, TaskPatch, TaskStatus};
use crate::service::work_service::{self, Allocation, SaveDay};
use crate::service::{ServiceError, agenda_service, task_service, undo_service};

/// 対応する MCP のプロトコルの版（新しい順）。クライアントが求めた版に対応していればそれを返す
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "個人の工数管理アプリ my_todo のタスクを扱う。\
タスクは木構造で、最上位は「タスク」（線表に載る）と「未分類」（とりあえず書き留める置き場）に分かれる。\
日付はすべて YYYY-MM-DD。工数は 1 日の勤務時間をタスクごとの割合で 0.5 時間刻みに振り分けて記録する。\
変更は画面の Ctrl+Z（または undo ツール）で元に戻せる。\
「今日やること」は get_schedule、思いついた ToDo は add_todo（既定で未分類に入る）を使う。";

pub fn run(conn: &Connection) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle_message(conn, &msg),
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("JSON として読めません: {e}"),
            )),
        };
        if let Some(resp) = response {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 1 メッセージを処理する。通知（id なし）には応答しない
pub fn handle_message(conn: &Connection, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // 通知（notifications/initialized など）
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => Ok(call_tool(conn, &params)),
        _ => Err((-32601, format!("未対応のメソッドです: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => error_response(id, code, &m),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == requested)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "my_todo", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

// ---------- ツールの定義 ----------
fn date_prop(desc: &str) -> Value {
    json!({ "type": ["string", "null"], "description": format!("{desc}（YYYY-MM-DD。null で取り消し）") })
}

str_enum! {
    /// 公開するツール。定義（tools/list）と実行（tools/call）の両方がこの enum の網羅的な match なので、
    /// ツールを足したときに片方を書き忘れるとコンパイルエラーになる
    pub enum Tool {
        GetSchedule => "get_schedule",
        ListTasks => "list_tasks",
        GetTask => "get_task",
        AddTodo => "add_todo",
        UpdateTask => "update_task",
        MoveTask => "move_task",
        DeleteTask => "delete_task",
        GetWorkDay => "get_work_day",
        LogWorkDay => "log_work_day",
        ListTags => "list_tags",
        Undo => "undo",
        Redo => "redo",
    }
}

/// タスクの項目（追加・更新で共通）
fn task_fields() -> serde_json::Map<String, Value> {
    let statuses: Vec<&str> = TaskStatus::ALL.iter().map(|s| s.as_str()).collect();
    let v = json!({
        "title": { "type": "string", "description": "タイトル" },
        "status": { "type": "string", "enum": statuses, "description": "todo=未着手 / doing=作業中 / done=完了" },
        "note": { "type": ["string", "null"], "description": "メモ（null か空文字で消す）" },
        "planned_start": date_prop("開始予定日"),
        "planned_end": date_prop("終了予定日"),
        "next_action": { "type": ["string", "null"], "description": "次にやること（例：課長にレビュー依頼）" },
        "next_action_date": date_prop("次にやる日"),
        "tags": { "type": "array", "items": { "type": "string" }, "description": "タグ（指定した一覧で置き換える）" },
        "estimate_hours": { "type": ["number", "null"], "description": "見積もり（時間）" },
    });
    match v {
        Value::Object(m) => m,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            serde_json::Map::new()
        }
    }
}

fn add_props() -> serde_json::Map<String, Value> {
    let mut m = task_fields();
    m.remove("status");
    m.insert(
        "parent_id".into(),
        json!({ "type": "integer", "description": "親タスクの id。指定するとその子になる" }),
    );
    m.insert("to_tasks".into(), json!({ "type": "boolean", "description": "true なら未分類ではなくタスク（線表）の最上位に入れる。既定 false" }));
    m
}

fn update_props() -> serde_json::Map<String, Value> {
    let mut m = task_fields();
    m.insert(
        "id".into(),
        json!({ "type": "integer", "description": "タスクの id" }),
    );
    m
}

impl Tool {
    /// tools/list に出す定義
    fn definition(self) -> Value {
        match self {
            Tool::GetSchedule => json!({
                "name": self.as_str(),
                "title": "今日・今週の予定",
                "description": "期限切れ・今日・今週（日曜まで）・来週の予定を返す。次の予定の日付・終了予定日・作業中のタスクから組み立てる。「今日やることは？」にはこれを使う。",
                "inputSchema": { "type": "object", "properties": {
                    "date": { "type": "string", "description": "基準日（YYYY-MM-DD）。省略で今日" }
                } },
                "annotations": { "readOnlyHint": true },
            }),
            Tool::ListTasks => json!({
                "name": self.as_str(),
                "title": "タスクの一覧",
                "description": "タスクを絞り込んで一覧する。既定では完了を除く。",
                "inputSchema": { "type": "object", "properties": {
                    "query": { "type": "string", "description": "タイトル・メモ・次の予定・タグに含まれる語（空白区切りですべて含む）" },
                    "status": { "type": "array", "items": { "type": "string", "enum": ["todo", "doing", "done"] }, "description": "状態で絞る" },
                    "tag": { "type": "string", "description": "このタグが自分か祖先に付いているもの" },
                    "section": { "type": "string", "enum": ["all", "tasks", "inbox"], "description": "tasks=線表 / inbox=未分類。既定 all" },
                    "parent_id": { "type": "integer", "description": "このタスクの子孫だけ" },
                    "include_done": { "type": "boolean", "description": "完了も含める。既定 false（status を指定したときは無視）" },
                    "limit": { "type": "integer", "description": "最大件数。既定 100" }
                } },
                "annotations": { "readOnlyHint": true },
            }),
            Tool::GetTask => json!({
                "name": self.as_str(),
                "title": "タスクの詳細",
                "description": "1 件のタスクの詳細（メモ・子タスク・変更履歴を含む）を返す。",
                "inputSchema": { "type": "object", "properties": {
                    "id": { "type": "integer", "description": "タスクの id" }
                }, "required": ["id"] },
                "annotations": { "readOnlyHint": true },
            }),
            Tool::AddTodo => json!({
                "name": self.as_str(),
                "title": "ToDo を追加",
                "description": "タスクを追加する。必須はタイトルだけ。既定では未分類に入る。",
                "inputSchema": { "type": "object", "properties": add_props(), "required": ["title"] },
            }),
            Tool::UpdateTask => json!({
                "name": self.as_str(),
                "title": "タスクを更新",
                "description": "タスクの項目を更新する。指定した項目だけ変わる。完了にするには status を done にする。",
                "inputSchema": { "type": "object", "properties": update_props(), "required": ["id"] },
            }),
            Tool::MoveTask => json!({
                "name": self.as_str(),
                "title": "タスクを移動",
                "description": "親の付け替えと並べ替え。parent_id を指定するとその子に、省略すると to で指定した最上位へ移す。",
                "inputSchema": { "type": "object", "properties": {
                    "id": { "type": "integer" },
                    "parent_id": { "type": "integer", "description": "新しい親の id" },
                    "to": { "type": "string", "enum": ["tasks", "inbox"], "description": "parent_id を省略したときの移動先。既定 tasks" },
                    "index": { "type": "integer", "description": "兄弟の中での位置（0 始まり）。省略で末尾" }
                }, "required": ["id"] },
            }),
            Tool::DeleteTask => json!({
                "name": self.as_str(),
                "title": "タスクを削除",
                "description": "タスクを子孫ごと削除する。undo で元に戻せる。",
                "inputSchema": { "type": "object", "properties": {
                    "id": { "type": "integer" }
                }, "required": ["id"] },
                "annotations": { "destructiveHint": true },
            }),
            Tool::GetWorkDay => json!({
                "name": self.as_str(),
                "title": "その日の工数",
                "description": "その日の勤務時間・タスクごとの振り分け・その日に関わったタスク（振り分けの候補）を返す。",
                "inputSchema": { "type": "object", "properties": {
                    "date": { "type": "string", "description": "YYYY-MM-DD。省略で今日" }
                } },
                "annotations": { "readOnlyHint": true },
            }),
            Tool::LogWorkDay => json!({
                "name": self.as_str(),
                "title": "工数を記録",
                "description": "その日の勤務時間（残業込みの実際の時間）と、タスクごとの割合を保存する。勤務時間を割合で 0.5 時間刻みに振り分け、合計は勤務時間に一致する。その日の振り分けはこの内容で置き換わる。例：10 時間を A:3, B:1 → A 7.5h, B 2.5h。",
                "inputSchema": { "type": "object", "properties": {
                    "date": { "type": "string", "description": "YYYY-MM-DD。省略で今日" },
                    "worked_hours": { "type": "number", "description": "勤務時間（0.5 時間刻み）" },
                    "allocations": { "type": "array", "description": "タスクごとの割合", "items": {
                        "type": "object",
                        "properties": { "task_id": { "type": "integer" }, "weight": { "type": "number", "description": "割合（0 以上。比で効く）" } },
                        "required": ["task_id", "weight"]
                    } }
                }, "required": ["worked_hours", "allocations"] },
            }),
            Tool::ListTags => json!({
                "name": self.as_str(),
                "title": "タグの一覧",
                "description": "使われているタグと件数。",
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": { "readOnlyHint": true },
            }),
            Tool::Undo => json!({
                "name": self.as_str(),
                "title": "元に戻す",
                "description": "直前の変更（画面・AI どちらの変更でも）を元に戻す。",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            Tool::Redo => json!({
                "name": self.as_str(),
                "title": "やり直し",
                "description": "元に戻した変更をやり直す。",
                "inputSchema": { "type": "object", "properties": {} },
            }),
        }
    }
}

fn tool_definitions() -> Value {
    Value::Array(Tool::ALL.iter().map(|t| t.definition()).collect())
}

// ---------- ツールの実行 ----------
fn call_tool(conn: &Connection, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match run_tool(conn, name, &args) {
        Ok(value) => {
            // MCP の仕様で structuredContent はオブジェクトでないといけない。
            // 配列などを返すツールを足しても弾かれないよう、オブジェクト以外は包む
            let value = if value.is_object() {
                value
            } else {
                json!({ "result": value })
            };
            json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
            "structuredContent": value,
            "isError": false,
            })
        }
        // ツールの失敗は JSON-RPC のエラーではなく isError で返す（AI が読んで直せるように）
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

type ToolResult = std::result::Result<Value, String>;

fn svc<T>(r: std::result::Result<T, ServiceError>) -> std::result::Result<T, String> {
    r.map_err(|e| e.to_string())
}

fn arg_i64(args: &Value, key: &str) -> std::result::Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{key} を整数で指定してください"))
}

fn arg_date(args: &Value) -> std::result::Result<String, String> {
    match args.get("date").and_then(Value::as_str) {
        Some(d) if !d.is_empty() => Ok(d.to_string()),
        _ => Ok(crate::service::today().to_string()),
    }
}

fn run_tool(conn: &Connection, name: &str, args: &Value) -> ToolResult {
    let tool = Tool::parse(name).ok_or_else(|| format!("未対応のツールです: {name}"))?;
    match tool {
        Tool::GetSchedule => {
            let date = arg_date(args)?;
            let today = svc(crate::service::parse_date(&date))?;
            let agenda = svc(agenda_service::agenda(conn, today))?;
            Ok(serde_json::to_value(agenda).unwrap())
        }
        Tool::ListTasks => list_tasks(conn, args),
        Tool::GetTask => {
            let id = arg_i64(args, "id")?;
            let tasks = svc(task_service::list_tasks(conn))?;
            let index = TaskIndex::new(&tasks);
            let task = index
                .by_id
                .get(&id)
                .ok_or_else(|| format!("タスクが見つかりません: {id}"))?;
            let mut v = index.summary(task, true);
            let children: Vec<Value> = tasks
                .iter()
                .filter(|t| t.parent_id == Some(id))
                .map(|t| index.summary(t, false))
                .collect();
            v["children"] = json!(children);
            v["history"] = serde_json::to_value(svc(task_service::list_events(conn, id))?).unwrap();
            Ok(v)
        }
        Tool::AddTodo => {
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let label = format!("AI:「{}」の追加", title.trim());
            let parent_id = args.get("parent_id").and_then(Value::as_i64);
            let to_tasks = args
                .get("to_tasks")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let patch = patch_from_args(args)?;
            let task = svc(undo_service::record(conn, &label, |c| {
                let t = task_service::create_task(
                    c,
                    NewTask {
                        title,
                        parent_id,
                        note: None,
                        inbox: Some(!to_tasks),
                    },
                )?;
                task_service::update_task(c, t.id, patch)
            }))?;
            task_result(conn, &task)
        }
        Tool::UpdateTask => {
            let id = arg_i64(args, "id")?;
            let current = svc(task_service::get_task(conn, id))?;
            let patch = patch_from_args(args)?;
            let label = format!("AI:「{}」の変更", current.title);
            let task = svc(undo_service::record(conn, &label, |c| {
                task_service::update_task(c, id, patch)
            }))?;
            task_result(conn, &task)
        }
        Tool::MoveTask => {
            let id = arg_i64(args, "id")?;
            let current = svc(task_service::get_task(conn, id))?;
            let target = MoveTarget {
                parent_id: args.get("parent_id").and_then(Value::as_i64),
                inbox: args.get("to").and_then(Value::as_str) == Some("inbox"),
                index: args
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|i| i as usize),
            };
            let label = format!("AI:「{}」の移動", current.title);
            let task = svc(undo_service::record(conn, &label, |c| {
                task_service::move_task(c, id, target)
            }))?;
            task_result(conn, &task)
        }
        Tool::DeleteTask => {
            let id = arg_i64(args, "id")?;
            let current = svc(task_service::get_task(conn, id))?;
            let label = format!("AI:「{}」の削除", current.title);
            svc(undo_service::record(conn, &label, |c| {
                task_service::delete_task(c, id)
            }))?;
            Ok(
                json!({ "deleted": id, "title": current.title, "hint": "undo ツールか画面の Ctrl+Z で元に戻せます" }),
            )
        }
        Tool::GetWorkDay => {
            let date = arg_date(args)?;
            work_day_result(conn, &date)
        }
        Tool::LogWorkDay => {
            let date = arg_date(args)?;
            let hours = args
                .get("worked_hours")
                .and_then(Value::as_f64)
                .ok_or("worked_hours を数値で指定してください")?;
            let allocations: Vec<Allocation> = args
                .get("allocations")
                .and_then(Value::as_array)
                .ok_or("allocations を配列で指定してください")?
                .iter()
                .map(|a| {
                    Ok(Allocation {
                        task_id: a
                            .get("task_id")
                            .and_then(Value::as_i64)
                            .ok_or("allocations の task_id がありません")?,
                        weight: a
                            .get("weight")
                            .and_then(Value::as_f64)
                            .ok_or("allocations の weight がありません")?,
                    })
                })
                .collect::<std::result::Result<_, &str>>()?;
            let input = SaveDay {
                worked_min: (hours * 60.0).round() as i64,
                allocations,
            };
            let label = format!("AI:{date} の工数");
            svc(undo_service::record(conn, &label, |c| {
                work_service::save_day(c, &date, input)
            }))?;
            work_day_result(conn, &date)
        }
        Tool::ListTags => Ok(json!({ "tags": svc(task_service::list_tags(conn))? })),
        Tool::Undo => Ok(json!({ "undone": svc(undo_service::undo(conn))? })),
        Tool::Redo => Ok(json!({ "redone": svc(undo_service::redo(conn))? })),
    }
}

/// 引数から TaskPatch を作る（見積もりは時間 → 分）
fn patch_from_args(args: &Value) -> std::result::Result<TaskPatch, String> {
    let mut obj = args.as_object().cloned().unwrap_or_default();
    for k in ["id", "parent_id", "to_tasks"] {
        obj.remove(k);
    }
    if let Some(h) = obj.remove("estimate_hours") {
        // null は「見積もりを消す」、数値は時間 → 分
        let min = if h.is_null() {
            Value::Null
        } else {
            let hours = h
                .as_f64()
                .ok_or("estimate_hours は数値で指定してください")?;
            json!((hours * 60.0).round() as i64)
        };
        obj.insert("estimate_min".into(), min);
    }
    serde_json::from_value(Value::Object(obj)).map_err(|e| format!("引数が正しくありません: {e}"))
}

fn task_result(conn: &Connection, task: &Task) -> ToolResult {
    let tasks = svc(task_service::list_tasks(conn))?;
    let index = TaskIndex::new(&tasks);
    let t = index.by_id.get(&task.id).copied().unwrap_or(task);
    Ok(index.summary(t, true))
}

fn work_day_result(conn: &Connection, date: &str) -> ToolResult {
    let day = svc(work_service::get_day(conn, date))?;
    let settings = svc(work_service::get_settings(conn))?;
    let tasks = svc(task_service::list_tasks(conn))?;
    let index = TaskIndex::new(&tasks);
    let title = |id: i64| {
        index
            .by_id
            .get(&id)
            .map(|t| index.path_title(t))
            .unwrap_or_default()
    };
    Ok(json!({
        "date": day.date,
        "worked_hours": day.worked_min.map(hours),
        "hours_per_day": hours(settings.minutes_per_day),
        "entries": day.entries.iter().map(|e| json!({
            "task_id": e.task_id, "task": title(e.task_id), "weight": e.weight, "hours": hours(e.minutes)
        })).collect::<Vec<_>>(),
        "suggestions": day.suggestions.iter().map(|s| json!({
            "task_id": s.task_id, "task": title(s.task_id), "reason": s.reason
        })).collect::<Vec<_>>(),
    }))
}

fn hours(min: i64) -> f64 {
    (min as f64 / 60.0 * 100.0).round() / 100.0
}

fn list_tasks(conn: &Connection, args: &Value) -> ToolResult {
    let tasks = svc(task_service::list_tasks(conn))?;
    let index = TaskIndex::new(&tasks);
    let statuses: Vec<String> = args
        .get("status")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let include_done = args
        .get("include_done")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tag = args
        .get("tag")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let section = args.get("section").and_then(Value::as_str).unwrap_or("all");
    let parent_id = args.get("parent_id").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let words: Vec<String> = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();

    let mut out = Vec::new();
    let mut total = 0;
    for t in tree_order(&tasks) {
        if !statuses.is_empty() {
            if !statuses.iter().any(|s| s == t.status.as_str()) {
                continue;
            }
        } else if !include_done && t.status == TaskStatus::Done {
            continue;
        }
        if section != "all" && (section == "inbox") != index.root(t).is_inbox {
            continue;
        }
        if let Some(p) = parent_id
            && (t.id == p || !index.ancestors(t).any(|a| a.id == p))
        {
            continue;
        }
        if let Some(tag) = &tag {
            let has = std::iter::once(t)
                .chain(index.ancestors(t))
                .any(|x| x.tags.iter().any(|n| n.to_lowercase() == *tag));
            if !has {
                continue;
            }
        }
        if !words.is_empty() {
            let hay = [
                Some(t.title.as_str()),
                t.note.as_deref(),
                t.next_action.as_deref(),
            ]
            .into_iter()
            .flatten()
            .chain(t.tags.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
            if !words.iter().all(|w| hay.contains(w)) {
                continue;
            }
        }
        total += 1;
        if out.len() < limit {
            out.push(index.summary(t, false));
        }
    }
    Ok(json!({ "total": total, "tasks": out }))
}

/// 画面と同じ見え方の順に並べる：線表のタスク → 未分類のタスク。それぞれ木の順（親のすぐ後にその子）
fn tree_order(tasks: &[Task]) -> Vec<&Task> {
    let mut children: HashMap<Option<i64>, Vec<&Task>> = HashMap::new();
    for t in tasks {
        children.entry(t.parent_id).or_default().push(t);
    }
    for list in children.values_mut() {
        list.sort_by_key(|t| (t.sort_order, t.id));
    }
    fn walk<'a>(
        t: &'a Task,
        children: &HashMap<Option<i64>, Vec<&'a Task>>,
        out: &mut Vec<&'a Task>,
    ) {
        out.push(t);
        for c in children.get(&Some(t.id)).into_iter().flatten() {
            walk(c, children, out);
        }
    }
    let mut out = Vec::with_capacity(tasks.len());
    let roots = children.get(&None).cloned().unwrap_or_default();
    // 線表（is_inbox = false）を先に、未分類を後に
    for inbox in [false, true] {
        for t in roots.iter().filter(|t| t.is_inbox == inbox) {
            walk(t, &children, &mut out);
        }
    }
    out
}

/// 親をたどったり、AI に見せる形にまとめたりするための索引
struct TaskIndex<'a> {
    by_id: HashMap<i64, &'a Task>,
    children: HashMap<i64, usize>,
}

impl<'a> TaskIndex<'a> {
    fn new(tasks: &'a [Task]) -> Self {
        let mut children = HashMap::new();
        for t in tasks {
            if let Some(p) = t.parent_id {
                *children.entry(p).or_insert(0) += 1;
            }
        }
        TaskIndex {
            by_id: tasks.iter().map(|t| (t.id, t)).collect(),
            children,
        }
    }

    fn ancestors(&self, t: &'a Task) -> impl Iterator<Item = &'a Task> + '_ {
        let mut cur = t.parent_id.and_then(|p| self.by_id.get(&p).copied());
        std::iter::from_fn(move || {
            let x = cur?;
            cur = x.parent_id.and_then(|p| self.by_id.get(&p).copied());
            Some(x)
        })
    }

    fn root(&self, t: &'a Task) -> &'a Task {
        self.ancestors(t).last().unwrap_or(t)
    }

    /// "親 › 子 › 自分"
    fn path_title(&self, t: &'a Task) -> String {
        let mut names: Vec<&str> = self.ancestors(t).map(|a| a.title.as_str()).collect();
        names.reverse();
        names.push(&t.title);
        names.join(" › ")
    }

    fn summary(&self, t: &'a Task, with_note: bool) -> Value {
        let mut parents: Vec<&str> = self.ancestors(t).map(|a| a.title.as_str()).collect();
        parents.reverse();
        let mut v = json!({
            "id": t.id,
            "title": t.title,
            "path": parents.join(" › "),
            "section": if self.root(t).is_inbox { "inbox" } else { "tasks" },
            "status": t.status,
            "tags": t.tags,
            "planned_start": t.planned_start,
            "planned_end": t.planned_end,
            "next_action": t.next_action,
            "next_action_date": t.next_action_date,
            "estimate_hours": t.estimate_min.map(hours),
            "actual_hours": hours(t.actual_min),
            "children": self.children.get(&t.id).copied().unwrap_or(0),
            "completed_at": t.completed_at,
        });
        if with_note {
            v["note"] = json!(t.note);
            v["parent_id"] = json!(t.parent_id);
            v["started_at"] = json!(t.started_at);
            v["created_at"] = json!(t.created_at);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn call(conn: &Connection, id: i64, method: &str, params: Value) -> Value {
        handle_message(
            conn,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .unwrap()
    }

    fn tool(conn: &Connection, name: &str, args: Value) -> Value {
        call(
            conn,
            1,
            "tools/call",
            json!({ "name": name, "arguments": args }),
        )["result"]
            .clone()
    }

    #[test]
    fn initialize_negotiates_version_and_ignores_notifications() {
        let conn = db::open_in_memory().unwrap();
        let r = call(
            &conn,
            1,
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = call(
            &conn,
            2,
            "initialize",
            json!({ "protocolVersion": "1999-01-01" }),
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(
            handle_message(
                &conn,
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .is_none()
        );
        assert_eq!(call(&conn, 3, "nope", json!({}))["error"]["code"], -32601);
    }

    #[test]
    fn tools_list_has_valid_schemas() {
        let conn = db::open_in_memory().unwrap();
        let r = call(&conn, 1, "tools/list", json!({}));
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 10);
        for t in tools {
            assert!(t["name"].is_string() && t["description"].is_string());
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn add_update_list_and_undo() {
        let conn = db::open_in_memory().unwrap();
        let r = tool(
            &conn,
            "add_todo",
            json!({ "title": "資料を読む", "next_action_date": "2026-11-21", "tags": ["勉強"], "estimate_hours": 2 }),
        );
        assert_eq!(r["isError"], false);
        let t = &r["structuredContent"];
        let id = t["id"].as_i64().unwrap();
        assert_eq!(t["section"], "inbox");
        assert_eq!(t["estimate_hours"], 2.0);
        assert_eq!(t["tags"], json!(["勉強"]));

        let r = tool(
            &conn,
            "update_task",
            json!({ "id": id, "status": "done", "next_action_date": null }),
        );
        assert_eq!(r["structuredContent"]["status"], "done");
        assert!(r["structuredContent"]["next_action_date"].is_null());

        let r = tool(&conn, "list_tasks", json!({}));
        assert_eq!(r["structuredContent"]["total"], 0); // 完了は既定で除く
        let r = tool(
            &conn,
            "list_tasks",
            json!({ "include_done": true, "query": "資料" }),
        );
        assert_eq!(r["structuredContent"]["total"], 1);

        // 画面と同じ元に戻すで戻る
        let r = tool(&conn, "undo", json!({}));
        assert_eq!(r["structuredContent"]["undone"], "AI:「資料を読む」の変更");
        assert_eq!(
            task_service::get_task(&conn, id).unwrap().status,
            TaskStatus::Todo
        );
    }

    /// MCP の仕様：structuredContent は必ずオブジェクト（全ツールで確かめる）
    #[test]
    fn structured_content_is_always_an_object() {
        let conn = db::open_in_memory().unwrap();
        let id = tool(&conn, "add_todo", json!({ "title": "a", "tags": ["x"] }))["structuredContent"]["id"]
            .as_i64()
            .unwrap();
        let args = |t: Tool| match t {
            Tool::GetTask | Tool::UpdateTask | Tool::MoveTask | Tool::DeleteTask => {
                json!({ "id": id })
            }
            Tool::AddTodo => json!({ "title": "b" }),
            Tool::LogWorkDay => {
                json!({ "date": "2026-11-20", "worked_hours": 8, "allocations": [] })
            }
            Tool::GetSchedule
            | Tool::ListTasks
            | Tool::GetWorkDay
            | Tool::ListTags
            | Tool::Undo
            | Tool::Redo => json!({}),
        };
        // 削除は最後に（ほかのツールが id を使うので）
        let mut tools: Vec<Tool> = Tool::ALL
            .iter()
            .copied()
            .filter(|t| *t != Tool::DeleteTask)
            .collect();
        tools.push(Tool::DeleteTask);
        for t in tools {
            let r = tool(&conn, t.as_str(), args(t));
            assert_eq!(r["isError"], false, "{}: {r}", t.as_str());
            assert!(
                r["structuredContent"].is_object(),
                "{} の structuredContent がオブジェクトでない: {r}",
                t.as_str()
            );
        }
        let tags = tool(&conn, "list_tags", json!({}));
        assert!(tags["structuredContent"]["tags"].is_array());
    }

    /// 一覧は画面と同じ順（線表 → 未分類、木の順）
    #[test]
    fn list_tasks_follows_screen_order() {
        let conn = db::open_in_memory().unwrap();
        let add = |args: Value| {
            tool(&conn, "add_todo", args)["structuredContent"]["id"]
                .as_i64()
                .unwrap()
        };
        add(json!({ "title": "未分類1" }));
        let p = add(json!({ "title": "線表1", "to_tasks": true }));
        add(json!({ "title": "未分類2" }));
        add(json!({ "title": "線表1の子", "parent_id": p }));
        add(json!({ "title": "線表2", "to_tasks": true }));
        let r = tool(&conn, "list_tasks", json!({}));
        let titles: Vec<&str> = r["structuredContent"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["線表1", "線表1の子", "線表2", "未分類1", "未分類2"]
        );
    }

    #[test]
    fn errors_come_back_as_tool_errors() {
        let conn = db::open_in_memory().unwrap();
        let r = tool(&conn, "update_task", json!({ "id": 999, "title": "x" }));
        assert_eq!(r["isError"], true);
        let r = tool(
            &conn,
            "add_todo",
            json!({ "title": "a", "planned_end": "11/30" }),
        );
        assert_eq!(r["isError"], true);
        // 失敗した追加は残らない
        assert!(task_service::list_tasks(&conn).unwrap().is_empty());
    }

    #[test]
    fn schedule_and_work_day() {
        let conn = db::open_in_memory().unwrap();
        let a = tool(
            &conn,
            "add_todo",
            json!({ "title": "A", "to_tasks": true, "next_action_date": "2026-11-21" }),
        );
        let a = a["structuredContent"]["id"].as_i64().unwrap();
        let b = tool(&conn, "add_todo", json!({ "title": "B", "parent_id": a }))["structuredContent"]["id"].as_i64().unwrap();
        let r = tool(&conn, "get_schedule", json!({ "date": "2026-11-21" }));
        assert_eq!(r["structuredContent"]["today_items"][0]["title"], "A");

        let r = tool(
            &conn,
            "log_work_day",
            json!({ "date": "2026-11-20", "worked_hours": 10,
            "allocations": [{ "task_id": a, "weight": 3 }, { "task_id": b, "weight": 1 }] }),
        );
        let e = &r["structuredContent"]["entries"];
        assert_eq!(e[0]["hours"], 7.5);
        assert_eq!(e[1]["hours"], 2.5);
        assert_eq!(e[1]["task"], "A › B");
    }
}
