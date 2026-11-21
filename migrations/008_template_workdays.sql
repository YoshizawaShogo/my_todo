-- テンプレートの日付を「平日（土日を除く）で何日目か」で持てるようにする。
-- 前からあるテンプレートは暦日のまま（calendar）、これから保存するものは平日（workday）
ALTER TABLE template ADD COLUMN offset_unit TEXT NOT NULL DEFAULT 'calendar';

-- 元に戻す用のトリガーに新しい列を足す
DROP TRIGGER undo_template_delete;
DROP TRIGGER undo_template_update;

CREATE TRIGGER undo_template_delete BEFORE DELETE ON template BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO template (id,name,created_at,offset_unit) VALUES (' || quote(old.id) || ',' || quote(old.name) || ',' || quote(old.created_at) || ',' || quote(old.offset_unit) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_template_update AFTER UPDATE ON template BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE template SET name=' || quote(old.name) || ',created_at=' || quote(old.created_at) || ',offset_unit=' || quote(old.offset_unit) || ' WHERE id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
