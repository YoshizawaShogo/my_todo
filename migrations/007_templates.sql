-- テンプレート：既存のタスク（子を含む）から作り、使うときに開始日を選んで日程をずらして作る。
-- 日付は「基準日から何日目か」で持つ（NULL なら日付なし）。元に戻すの対象にする
CREATE TABLE template (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE template_item (
    id                 INTEGER PRIMARY KEY,
    template_id        INTEGER NOT NULL REFERENCES template(id) ON DELETE CASCADE,
    parent_item_id     INTEGER REFERENCES template_item(id) ON DELETE CASCADE,
    sort_order         INTEGER NOT NULL DEFAULT 0,
    title              TEXT NOT NULL,
    note               TEXT,
    tags               TEXT NOT NULL DEFAULT '[]', -- タグ名の JSON 配列
    estimate_min       INTEGER,
    start_offset       INTEGER,
    end_offset         INTEGER,
    next_action        TEXT,
    next_action_offset INTEGER
);
CREATE INDEX template_item_template_idx ON template_item(template_id, parent_item_id, sort_order);

CREATE TRIGGER undo_template_insert AFTER INSERT ON template BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM template WHERE id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_delete BEFORE DELETE ON template BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO template (id,name,created_at) VALUES (' || quote(old.id) || ',' || quote(old.name) || ',' || quote(old.created_at) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_update AFTER UPDATE ON template BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE template SET name=' || quote(old.name) || ',created_at=' || quote(old.created_at) || ' WHERE id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_item_insert AFTER INSERT ON template_item BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM template_item WHERE id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_item_delete BEFORE DELETE ON template_item BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO template_item (id,template_id,parent_item_id,sort_order,title,note,tags,estimate_min,start_offset,end_offset,next_action,next_action_offset) VALUES (' || quote(old.id) || ',' || quote(old.template_id) || ',' || quote(old.parent_item_id) || ',' || quote(old.sort_order) || ',' || quote(old.title) || ',' || quote(old.note) || ',' || quote(old.tags) || ',' || quote(old.estimate_min) || ',' || quote(old.start_offset) || ',' || quote(old.end_offset) || ',' || quote(old.next_action) || ',' || quote(old.next_action_offset) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_item_update AFTER UPDATE ON template_item BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE template_item SET template_id=' || quote(old.template_id) || ',parent_item_id=' || quote(old.parent_item_id) || ',sort_order=' || quote(old.sort_order) || ',title=' || quote(old.title) || ',note=' || quote(old.note) || ',tags=' || quote(old.tags) || ',estimate_min=' || quote(old.estimate_min) || ',start_offset=' || quote(old.start_offset) || ',end_offset=' || quote(old.end_offset) || ',next_action=' || quote(old.next_action) || ',next_action_offset=' || quote(old.next_action_offset) || ' WHERE id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
