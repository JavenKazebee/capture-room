CREATE TABLE IF NOT EXISTS node_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS recording_sessions (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL,
    preset_id       TEXT NOT NULL,
    started_at      TEXT NOT NULL,
    stopped_at      TEXT,
    primary_path    TEXT NOT NULL,
    secondary_path  TEXT,
    redundant_path  TEXT,
    status          TEXT NOT NULL,
    error_message   TEXT
);

CREATE TABLE IF NOT EXISTS presets_cache (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    data      TEXT NOT NULL,
    version   INTEGER NOT NULL,
    synced_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schedules_cache (
    id        TEXT PRIMARY KEY,
    data      TEXT NOT NULL,
    synced_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS benchmark_results (
    id        TEXT PRIMARY KEY,
    run_at    TEXT NOT NULL,
    profile   TEXT NOT NULL,
    max_feeds INTEGER NOT NULL,
    metrics   TEXT NOT NULL
);
