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

str_enum! {
    /// タスクの状態
    pub enum TaskStatus {
        /// 未着手
        Todo => "todo",
        /// 作業中
        Doing => "doing",
        /// 完了
        Done => "done",
    }
}

str_enum! {
    /// タスクの種類（自分のタスクか、外部ジョブか。外部ジョブはいまはタグで代用していて未使用）
    pub enum TaskKind {
        Own => "self",
        External => "external",
    }
}

str_enum! {
    /// 変更履歴（task_event）の種類
    pub enum EventKind {
        Created => "created",
        Title => "title",
        Status => "status",
        PlannedStart => "planned_start",
        PlannedEnd => "planned_end",
        NextAction => "next_action",
        Tags => "tags",
        Estimate => "estimate",
        Jira => "jira",
    }
}

/// タスクの関連資料（URL と表示名）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskLink {
    pub id: i64,
    pub url: String,
    pub title: String,
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
    /// 紐付けた Jira の課題キー（例：ABC-123）
    pub jira_key: Option<String>,
    /// 関連資料（URL）
    pub links: Vec<TaskLink>,
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
    /// Jira の課題キー（空文字か null で紐付けを外す）
    #[serde(default, deserialize_with = "double_option")]
    pub jira_key: Option<Option<String>>,
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
    pub kind: EventKind,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// まとめて移動するときの置き場所
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// ref_id の前に（並びを保って）
    Before { ref_id: i64 },
    /// ref_id の後ろに（並びを保って）
    After { ref_id: i64 },
    /// 親の子の末尾か、最上位（未分類・タスク）の末尾に
    Into {
        #[serde(default)]
        parent_id: Option<i64>,
        #[serde(default)]
        inbox: bool,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// すべての種類が、文字列 → 読み戻しで同じものに戻る（SQLite と JSON の両方）
    #[test]
    fn str_enums_round_trip() {
        let conn = Connection::open_in_memory().unwrap();
        for s in TaskStatus::ALL {
            let back: TaskStatus = conn.query_row("SELECT ?1", [s], |r| r.get(0)).unwrap();
            assert_eq!(back, *s);
            let json = serde_json::to_string(s).unwrap();
            assert_eq!(serde_json::from_str::<TaskStatus>(&json).unwrap(), *s);
        }
        for k in EventKind::ALL {
            assert_eq!(EventKind::parse(k.as_str()), Some(*k));
        }
        for k in TaskKind::ALL {
            assert_eq!(TaskKind::parse(k.as_str()), Some(*k));
        }
    }

    /// 知らない値は黙って既定値にせず、エラーにする
    #[test]
    fn unknown_values_are_errors() {
        let conn = Connection::open_in_memory().unwrap();
        let r: rusqlite::Result<EventKind> = conn.query_row("SELECT 'renamed'", [], |r| r.get(0));
        assert!(r.is_err());
        let e = serde_json::from_str::<TaskStatus>(r#""finished""#).unwrap_err();
        assert!(e.to_string().contains("todo / doing / done"), "{e}");
    }
}
