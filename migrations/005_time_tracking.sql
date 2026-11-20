-- フェーズ 2：工数の記録
-- 1 日の勤務時間をタスクに割合で振り分ける。時間は分で持ち、表示で時間・人日に換算する。

-- その日の勤務時間（残業も含めた実際の時間）
CREATE TABLE work_day (
    date       TEXT PRIMARY KEY,
    worked_min INTEGER NOT NULL,
    note       TEXT
);

-- 工数の実績。source = 'daily' は 1 日の割合での振り分け（weight が割合）。
-- 'timer' / 'manual' は将来のタイマー・手入力用。jira_worklog_id は Jira への二重送信防止用
CREATE TABLE time_entry (
    id              INTEGER PRIMARY KEY,
    task_id         INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    work_date       TEXT NOT NULL,
    minutes         INTEGER NOT NULL,
    weight          REAL,
    source          TEXT NOT NULL DEFAULT 'daily' CHECK (source IN ('daily', 'timer', 'manual')),
    started_at      TEXT,
    ended_at        TEXT,
    jira_worklog_id TEXT,
    created_at      TEXT NOT NULL
);
CREATE INDEX time_entry_date_idx ON time_entry(work_date);
CREATE INDEX time_entry_task_idx ON time_entry(task_id);
CREATE UNIQUE INDEX time_entry_daily_idx ON time_entry(work_date, task_id) WHERE source = 'daily';

-- 設定（1D の時間、標準の勤務時間など）
CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO setting (key, value) VALUES ('minutes_per_day', '480'), ('default_work_min', '480');

CREATE TRIGGER undo_work_day_insert AFTER INSERT ON work_day BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM work_day WHERE ' || 'date=' || quote(new.date)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_work_day_delete BEFORE DELETE ON work_day BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO work_day (date,worked_min,note) VALUES (' || quote(old.date) || ',' || quote(old.worked_min) || ',' || quote(old.note) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_work_day_update AFTER UPDATE ON work_day BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE work_day SET ' || 'worked_min=' || quote(old.worked_min) || ',' || 'note=' || quote(old.note) || ' WHERE ' || 'date=' || quote(old.date)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_time_entry_insert AFTER INSERT ON time_entry BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM time_entry WHERE ' || 'id=' || quote(new.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_time_entry_delete BEFORE DELETE ON time_entry BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO time_entry (id,task_id,work_date,minutes,weight,source,started_at,ended_at,jira_worklog_id,created_at) VALUES (' || quote(old.id) || ',' || quote(old.task_id) || ',' || quote(old.work_date) || ',' || quote(old.minutes) || ',' || quote(old.weight) || ',' || quote(old.source) || ',' || quote(old.started_at) || ',' || quote(old.ended_at) || ',' || quote(old.jira_worklog_id) || ',' || quote(old.created_at) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_time_entry_update AFTER UPDATE ON time_entry BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE time_entry SET ' || 'task_id=' || quote(old.task_id) || ',' || 'work_date=' || quote(old.work_date) || ',' || 'minutes=' || quote(old.minutes) || ',' || 'weight=' || quote(old.weight) || ',' || 'source=' || quote(old.source) || ',' || 'started_at=' || quote(old.started_at) || ',' || 'ended_at=' || quote(old.ended_at) || ',' || 'jira_worklog_id=' || quote(old.jira_worklog_id) || ',' || 'created_at=' || quote(old.created_at) || ' WHERE ' || 'id=' || quote(old.id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_setting_insert AFTER INSERT ON setting BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM setting WHERE ' || 'key=' || quote(new.key)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_setting_delete BEFORE DELETE ON setting BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO setting (key,value) VALUES (' || quote(old.key) || ',' || quote(old.value) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_setting_update AFTER UPDATE ON setting BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE setting SET ' || 'value=' || quote(old.value) || ' WHERE ' || 'key=' || quote(old.key)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
