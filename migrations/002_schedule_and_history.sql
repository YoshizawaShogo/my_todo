-- 終了予定日（planned_end を使う）に加えて、次の予定と着手日時、変更履歴を持つ
ALTER TABLE task ADD COLUMN started_at TEXT;
ALTER TABLE task ADD COLUMN next_action TEXT;
ALTER TABLE task ADD COLUMN next_action_date TEXT;

-- いつ何を決めたか・変えたかの自動記録。予定の立て直しを振り返るために使う
CREATE TABLE task_event (
    id        INTEGER PRIMARY KEY,
    task_id   INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    at        TEXT NOT NULL,
    kind      TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT
);

CREATE INDEX task_event_task_idx ON task_event(task_id, at);

-- 既存タスクの登録記録を補う
INSERT INTO task_event (task_id, at, kind, new_value)
SELECT id, created_at, 'created', title FROM task;
