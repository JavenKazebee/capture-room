-- The media library: files on this node that channels can play, probed when
-- added. Removing an entry leaves the file on disk.
CREATE TABLE media (
    id         TEXT PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    info       TEXT NOT NULL,
    origin     TEXT NOT NULL,
    session_id TEXT,
    added_at   TEXT NOT NULL
);
