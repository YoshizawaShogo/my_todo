use serde::{Deserialize, Deserializer, Serialize};

/// 「項目なし」と「null（消す）」を区別するためのデシリアライザ。
/// 項目がなければ None、null なら Some(None)、値があれば Some(Some(v))。
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::Doing => "doing",
            TaskStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(TaskStatus::Todo),
            "doing" => Some(TaskStatus::Doing),
            "done" => Some(TaskStatus::Done),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    #[serde(rename = "self")]
    Own,
    External,
}

impl TaskKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "self" => Some(TaskKind::Own),
            "external" => Some(TaskKind::External),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Task {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub kind: TaskKind,
    pub external_job_id: Option<i64>,
    pub status: TaskStatus,
    pub is_inbox: bool,
    pub sort_order: i64,
    pub planned_start: Option<String>,
    pub planned_end: Option<String>,
    pub estimate_min: Option<i64>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    /// 初めて作業中にした日時（自動記録）
    pub started_at: Option<String>,
    /// 次にやること（例：「課長にレビュー依頼」）
    pub next_action: Option<String>,
    /// 次にやる日（YYYY-MM-DD）
    pub next_action_date: Option<String>,
    /// タグ（名前順）
    pub tags: Vec<String>,
    /// 記録した工数の合計（分。自分の分だけで、子は含まない）
    pub actual_min: i64,
}

/// 登録に必要なのはタイトルだけ。親を指定しなければ未分類に入る。
#[derive(Debug, Default, Deserialize)]
pub struct NewTask {
    pub title: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
    /// 親なしのとき、未分類（true, 既定）かタスク一覧（false）か
    #[serde(default)]
    pub inbox: Option<bool>,
}

/// 指定した項目だけを更新する。文字列は空文字か null で削除。日付は YYYY-MM-DD。
#[derive(Debug, Default, Deserialize)]
pub struct TaskPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default, deserialize_with = "double_option")]
    pub note: Option<Option<String>>,
    /// 開始予定日
    #[serde(default, deserialize_with = "double_option")]
    pub planned_start: Option<Option<String>>,
    /// 終了予定日
    #[serde(default, deserialize_with = "double_option")]
    pub planned_end: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub next_action: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub next_action_date: Option<Option<String>>,
    /// タグを指定した一覧で置き換える
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// 見積もり（分）
    #[serde(default, deserialize_with = "double_option")]
    pub estimate_min: Option<Option<i64>>,
}

/// タグと、それが付いているタスクの数
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagCount {
    pub name: String,
    pub count: i64,
}

/// タスクの変更履歴（自動記録）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskEvent {
    pub id: i64,
    pub task_id: i64,
    pub at: String,
    /// created / title / status / planned_start / planned_end / next_action / tags / estimate
    pub kind: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// 移動先。parent_id が None のときは inbox で未分類かタスク一覧のどちらの最上位かを選ぶ。
#[derive(Debug, Default, Deserialize)]
pub struct MoveTarget {
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub inbox: bool,
    /// 兄弟の中での位置（0 始まり）。省略または範囲外なら末尾。
    #[serde(default)]
    pub index: Option<usize>,
}
