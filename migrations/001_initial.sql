-- フェーズ 1：タスクの木構造と外部ジョブの種類
CREATE TABLE external_job (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE task (
    id              INTEGER PRIMARY KEY,
    parent_id       INTEGER REFERENCES task(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'self' CHECK (kind IN ('self', 'external')),
    external_job_id INTEGER REFERENCES external_job(id) ON DELETE SET NULL,
    status          TEXT NOT NULL DEFAULT 'todo' CHECK (status IN ('todo', 'doing', 'done')),
    -- 親を持たないタスクのうち、未整理のものを受信箱に置く
    is_inbox        INTEGER NOT NULL DEFAULT 0,
    sort_order      INTEGER NOT NULL DEFAULT 0,
    planned_start   TEXT,
    planned_end     TEXT,
    estimate_min    INTEGER,
    note            TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    completed_at    TEXT
);

CREATE INDEX task_parent_idx ON task(parent_id, sort_order);
