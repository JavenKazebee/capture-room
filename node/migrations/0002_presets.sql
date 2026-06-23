-- Authoritative preset store. Authored on an aggregator (control station) and
-- synced down to nodes' presets_cache for recording-time resolution.
CREATE TABLE IF NOT EXISTS presets (
    id                        TEXT PRIMARY KEY,
    name                      TEXT NOT NULL,
    codec                     TEXT NOT NULL,
    container                 TEXT NOT NULL,
    resolution                TEXT,           -- "1920x1080"; null = match source
    framerate                 TEXT,           -- "30" or "30000/1001"; null = match source
    bitrate_kbps              INTEGER,        -- null = quality-based
    quality                   TEXT,
    output_template           TEXT NOT NULL,
    secondary_output_template TEXT,
    redundant_output_template TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    version                   INTEGER NOT NULL DEFAULT 1
);
