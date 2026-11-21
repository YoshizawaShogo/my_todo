-- フェーズ 5：Jira 連携
-- タスクと Jira 課題の紐付け（元に戻せる）
CREATE TABLE jira_link (
    task_id   INTEGER PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    issue_key TEXT NOT NULL
);
CREATE INDEX jira_link_issue_idx ON jira_link(issue_key);

-- Jira に送った作業記録（worklog）。Jira 側の状態を写したものなので、元に戻すの対象にしない。
-- 日ごと・課題ごとに 1 件。工数が変わったら更新、0 になったら削除して二重送信を防ぐ
CREATE TABLE jira_worklog (
    work_date  TEXT NOT NULL,
    issue_key  TEXT NOT NULL,
    worklog_id TEXT NOT NULL,
    minutes    INTEGER NOT NULL,
    sent_at    TEXT NOT NULL,
    PRIMARY KEY (work_date, issue_key)
);

CREATE TRIGGER undo_jira_link_insert AFTER INSERT ON jira_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'DELETE FROM jira_link WHERE task_id=' || quote(new.task_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_jira_link_delete BEFORE DELETE ON jira_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'INSERT INTO jira_link (task_id,issue_key) VALUES (' || quote(old.task_id) || ',' || quote(old.issue_key) || ')'
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;

CREATE TRIGGER undo_jira_link_update AFTER UPDATE ON jira_link BEGIN
    INSERT INTO undo_journal (stack, step, sql)
    SELECT CASE mode WHEN 'undo' THEN 'redo' ELSE 'undo' END, step, 'UPDATE jira_link SET issue_key=' || quote(old.issue_key) || ' WHERE task_id=' || quote(old.task_id)
    FROM undo_state WHERE id = 1 AND mode IS NOT NULL;
END;
