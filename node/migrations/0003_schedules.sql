-- Schedules start and stop recordings and playout at set times (controller only).
CREATE TABLE schedules (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    kind        TEXT NOT NULL,
    date        TEXT,
    weekdays    INTEGER NOT NULL DEFAULT 0,
    start_time  TEXT NOT NULL,
    duration_s  INTEGER,
    actions     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- One row per occurrence that came due: run, failed or skipped. The unique
-- key keeps an occurrence from running twice.
CREATE TABLE schedule_runs (
    id            TEXT PRIMARY KEY,
    schedule_id   TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    planned_start TEXT NOT NULL,
    planned_end   TEXT,
    started_at    TEXT,
    stopped_at    TEXT,
    status        TEXT NOT NULL,
    targets       TEXT NOT NULL,
    UNIQUE (schedule_id, planned_start)
);
CREATE INDEX schedule_runs_by_schedule ON schedule_runs (schedule_id, planned_start);
