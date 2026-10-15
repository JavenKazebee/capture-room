-- Sources that keep a replay buffer, and how. A source that's gone keeps
-- its row (an NDI source can come back); deleting a configured source
-- removes it.
CREATE TABLE replay_buffers (
    source_id TEXT PRIMARY KEY,
    codec     TEXT NOT NULL,
    minutes   INTEGER NOT NULL
);
