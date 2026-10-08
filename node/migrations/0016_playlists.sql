-- Each playout channel's playlist. Items go with their media entry; the
-- channel's rows are removed with the channel.
CREATE TABLE playlists (
    channel_id    TEXT PRIMARY KEY,
    loop_playlist INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE playlist_items (
    id         TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    position   INTEGER NOT NULL,
    media_id   TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    in_ms      INTEGER NOT NULL,
    out_ms     INTEGER,
    end_action TEXT NOT NULL
);

CREATE INDEX playlist_items_channel ON playlist_items(channel_id, position);
