-- 依存関係：pre_id（先に終わらせるタスク）が終わってから post_id（後のタスク）を始める。元に戻すの対象
CREATE TABLE task_dependency (
    pre_id  INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    post_id INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    PRIMARY KEY (pre_id, post_id),
    CHECK (pre_id <> post_id)
);
CREATE INDEX task_dependency_post_idx ON task_dependency(post_id);

CREATE TRIGGER undo_task_dependency_insert AFTER INSERT ON task_dependency BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task_dependency WHERE pre_id=' || quote(new.pre_id) || ' AND post_id=' || quote(new.post_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_dependency_delete BEFORE DELETE ON task_dependency BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task_dependency (pre_id,post_id) VALUES (' || quote(old.pre_id) || ',' || quote(old.post_id) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
