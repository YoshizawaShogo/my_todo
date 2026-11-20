-- 元に戻す・やり直し（Ctrl+Z / Ctrl+Y）
-- 変更のたびに、その変更を打ち消す SQL をトリガーで undo_journal に書き留める。
-- 方式は SQLite 公式の「Automatic Undo/Redo Using SQLite」と同じ。
CREATE TABLE undo_journal (
    seq   INTEGER PRIMARY KEY,
    stack TEXT NOT NULL CHECK (stack IN ('undo', 'redo')),
    step  INTEGER NOT NULL,
    sql   TEXT NOT NULL
);
CREATE INDEX undo_journal_step_idx ON undo_journal(stack, step);

-- 1 回の操作（1 ステップ）の名前。「元に戻しました：〜」の表示に使う
CREATE TABLE undo_step (
    step  INTEGER PRIMARY KEY,
    label TEXT NOT NULL
);

-- mode: NULL = 記録しない / 'record' = 通常の操作 / 'undo' = 元に戻し中 / 'redo' = やり直し中
CREATE TABLE undo_state (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT,
    step INTEGER
);
INSERT INTO undo_state (id, mode, step) VALUES (1, NULL, NULL);

CREATE TRIGGER undo_task_insert AFTER INSERT ON task BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task WHERE ' || 'id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_delete BEFORE DELETE ON task BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task (id,parent_id,title,kind,external_job_id,status,is_inbox,sort_order,planned_start,planned_end,estimate_min,note,created_at,updated_at,completed_at,started_at,next_action,next_action_date) VALUES (' || quote(old.id) || ',' || quote(old.parent_id) || ',' || quote(old.title) || ',' || quote(old.kind) || ',' || quote(old.external_job_id) || ',' || quote(old.status) || ',' || quote(old.is_inbox) || ',' || quote(old.sort_order) || ',' || quote(old.planned_start) || ',' || quote(old.planned_end) || ',' || quote(old.estimate_min) || ',' || quote(old.note) || ',' || quote(old.created_at) || ',' || quote(old.updated_at) || ',' || quote(old.completed_at) || ',' || quote(old.started_at) || ',' || quote(old.next_action) || ',' || quote(old.next_action_date) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_update AFTER UPDATE ON task BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE task SET ' || 'parent_id=' || quote(old.parent_id) || ',' || 'title=' || quote(old.title) || ',' || 'kind=' || quote(old.kind) || ',' || 'external_job_id=' || quote(old.external_job_id) || ',' || 'status=' || quote(old.status) || ',' || 'is_inbox=' || quote(old.is_inbox) || ',' || 'sort_order=' || quote(old.sort_order) || ',' || 'planned_start=' || quote(old.planned_start) || ',' || 'planned_end=' || quote(old.planned_end) || ',' || 'estimate_min=' || quote(old.estimate_min) || ',' || 'note=' || quote(old.note) || ',' || 'created_at=' || quote(old.created_at) || ',' || 'updated_at=' || quote(old.updated_at) || ',' || 'completed_at=' || quote(old.completed_at) || ',' || 'started_at=' || quote(old.started_at) || ',' || 'next_action=' || quote(old.next_action) || ',' || 'next_action_date=' || quote(old.next_action_date) || ' WHERE ' || 'id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_event_insert AFTER INSERT ON task_event BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task_event WHERE ' || 'id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_event_delete BEFORE DELETE ON task_event BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task_event (id,task_id,at,kind,old_value,new_value) VALUES (' || quote(old.id) || ',' || quote(old.task_id) || ',' || quote(old.at) || ',' || quote(old.kind) || ',' || quote(old.old_value) || ',' || quote(old.new_value) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_event_update AFTER UPDATE ON task_event BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE task_event SET ' || 'task_id=' || quote(old.task_id) || ',' || 'at=' || quote(old.at) || ',' || 'kind=' || quote(old.kind) || ',' || 'old_value=' || quote(old.old_value) || ',' || 'new_value=' || quote(old.new_value) || ' WHERE ' || 'id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_tag_insert AFTER INSERT ON tag BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM tag WHERE ' || 'id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_tag_delete BEFORE DELETE ON tag BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO tag (id,name) VALUES (' || quote(old.id) || ',' || quote(old.name) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_tag_update AFTER UPDATE ON tag BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE tag SET ' || 'name=' || quote(old.name) || ' WHERE ' || 'id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_tag_insert AFTER INSERT ON task_tag BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task_tag WHERE ' || 'task_id=' || quote(new.task_id) || ' AND ' || 'tag_id=' || quote(new.tag_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_tag_delete BEFORE DELETE ON task_tag BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task_tag (task_id,tag_id) VALUES (' || quote(old.task_id) || ',' || quote(old.tag_id) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
