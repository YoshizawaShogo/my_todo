-- 振り返り：完了したタスクごとの、見積もりと実績のずれの理由（候補から複数）と自由記述。元に戻すの対象
CREATE TABLE task_review (
    task_id    INTEGER PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    reasons    TEXT NOT NULL DEFAULT '[]', -- 理由の候補（ReviewReason の値の JSON 配列）
    note       TEXT,
    updated_at TEXT NOT NULL
);

CREATE TRIGGER undo_task_review_insert AFTER INSERT ON task_review BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task_review WHERE task_id=' || quote(new.task_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_review_delete BEFORE DELETE ON task_review BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task_review (task_id,reasons,note,updated_at) VALUES (' || quote(old.task_id) || ',' || quote(old.reasons) || ',' || quote(old.note) || ',' || quote(old.updated_at) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_review_update AFTER UPDATE ON task_review BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE task_review SET reasons=' || quote(old.reasons) || ',note=' || quote(old.note) || ',updated_at=' || quote(old.updated_at) || ' WHERE task_id=' || quote(old.task_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
