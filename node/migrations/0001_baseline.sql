CREATE TABLE IF NOT EXISTS node_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS recording_sessions (
    id            TEXT PRIMARY KEY,
    source_id     TEXT NOT NULL,
    preset_id     TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    stopped_at    TEXT,
    output_paths  TEXT NOT NULL DEFAULT '[]', -- JSON array of file paths, ordered by preset_outputs.sort_order
    status        TEXT NOT NULL,
    error_message TEXT
);

CREATE TABLE IF NOT EXISTS presets (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    version    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS preset_outputs (
    id           TEXT PRIMARY KEY,
    preset_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    codec        TEXT NOT NULL,
    container    TEXT NOT NULL,
    resolution   TEXT,               -- "1920x1080"; null = match source
    framerate    TEXT,               -- "30" or "30000/1001"; null = match source
    bitrate_kbps INTEGER,            -- null = quality-based
    quality      TEXT,
    path_template TEXT NOT NULL,
    sort_order   INTEGER NOT NULL DEFAULT 0
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

CREATE TABLE IF NOT EXISTS test_sources (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    pattern      TEXT NOT NULL DEFAULT 'smpte',
    width        INTEGER NOT NULL DEFAULT 1920,
    height       INTEGER NOT NULL DEFAULT 1080,
    fps_num      INTEGER NOT NULL DEFAULT 30,
    fps_den      INTEGER NOT NULL DEFAULT 1,
    audio_signal TEXT NOT NULL DEFAULT 'tone',
    frequency    REAL NOT NULL DEFAULT 440.0,
    channels     INTEGER NOT NULL DEFAULT 2,
    created_at   TEXT NOT NULL
);

INSERT OR IGNORE INTO test_sources (id, name, pattern, width, height, fps_num, fps_den, audio_signal, frequency, channels, created_at)
VALUES
    ('test-1', 'Test Source 1', 'smpte', 1920, 1080, 30, 1, 'tone',       440.0, 2, datetime('now')),
    ('test-2', 'Test Source 2', 'ball',  1920, 1080, 30, 1, 'pink-noise',   0.0, 2, datetime('now'));
