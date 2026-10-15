-- Saved replay clips: part of a replay buffer kept as the clip's own files
-- (hard links to the buffer's segments). Positions count from the start of
-- its first file, at node clock time base_us when it was saved.
CREATE TABLE replay_clips (
    id            TEXT PRIMARY KEY,
    source_id     TEXT NOT NULL,
    source_name   TEXT NOT NULL,
    name          TEXT NOT NULL,
    codec         TEXT NOT NULL,
    files         TEXT NOT NULL,
    base_us       INTEGER NOT NULL,
    from_us       INTEGER NOT NULL,
    to_us         INTEGER NOT NULL,
    utc_offset_us INTEGER,
    bytes         INTEGER NOT NULL,
    created_at    TEXT NOT NULL
);
