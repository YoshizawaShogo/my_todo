-- 関連資料：タスクに URL を付ける。title は表示名（既定はリンク先のページの名前。あとから変えられる）
CREATE TABLE task_link (
    id         INTEGER PRIMARY KEY,
    task_id    INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    url        TEXT NOT NULL,
    title      TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX task_link_task_idx ON task_link(task_id, sort_order);

CREATE TRIGGER undo_task_link_insert AFTER INSERT ON task_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM task_link WHERE id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_link_delete BEFORE DELETE ON task_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO task_link (id,task_id,sort_order,url,title,created_at) VALUES (' || quote(old.id) || ',' || quote(old.task_id) || ',' || quote(old.sort_order) || ',' || quote(old.url) || ',' || quote(old.title) || ',' || quote(old.created_at) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_task_link_update AFTER UPDATE ON task_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE task_link SET task_id=' || quote(old.task_id) || ',sort_order=' || quote(old.sort_order) || ',url=' || quote(old.url) || ',title=' || quote(old.title) || ',created_at=' || quote(old.created_at) || ' WHERE id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
