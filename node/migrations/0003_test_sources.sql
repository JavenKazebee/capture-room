CREATE TABLE IF NOT EXISTS test_sources (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    pattern      TEXT NOT NULL DEFAULT 'smpte',    -- videotestsrc pattern
    width        INTEGER NOT NULL DEFAULT 1920,
    height       INTEGER NOT NULL DEFAULT 1080,
    fps_num      INTEGER NOT NULL DEFAULT 30,
    fps_den      INTEGER NOT NULL DEFAULT 1,
    audio_signal TEXT NOT NULL DEFAULT 'tone',     -- tone | silence | pink-noise
    frequency    REAL NOT NULL DEFAULT 440.0,
    channels     INTEGER NOT NULL DEFAULT 2,
    created_at   TEXT NOT NULL
);

-- Seed two defaults so a fresh install has something to show.
INSERT OR IGNORE INTO test_sources (id, name, pattern, width, height, fps_num, fps_den, audio_signal, frequency, channels, created_at)
VALUES
    ('test-1', 'Test Source 1', 'smpte', 1920, 1080, 30, 1, 'tone',      440.0, 2, datetime('now')),
    ('test-2', 'Test Source 2', 'ball',  1920, 1080, 30, 1, 'pink-noise', 0.0,  2, datetime('now'));
