-- The whole schema, squashed from the development migrations before the first
-- release. Add changes as new numbered migrations after this one.

CREATE TABLE node_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Nodes registered manually on a controller (mDNS-discovered nodes are not
-- persisted; they re-announce themselves).
CREATE TABLE nodes (
    id       TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    url      TEXT NOT NULL,
    added_at TEXT NOT NULL
);

CREATE TABLE presets (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    version    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE preset_outputs (
    id            TEXT PRIMARY KEY,
    preset_id     TEXT NOT NULL REFERENCES presets(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    codec         TEXT NOT NULL,
    container     TEXT NOT NULL,
    resolution    TEXT,               -- "1920x1080"; null = match source
    framerate     TEXT,               -- "30" or "30000/1001"; null = match source
    bitrate_kbps  INTEGER,            -- null = encoder default
    chroma        TEXT NOT NULL DEFAULT '420', -- "420" | "422" | "444"
    path_template TEXT NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    advanced      TEXT NOT NULL DEFAULT '{}', -- `OutputAdvanced` JSON; '{}' is all defaults
    preview       INTEGER NOT NULL DEFAULT 0  -- the output to preview a recording with
);

CREATE INDEX preset_outputs_preset_id ON preset_outputs (preset_id);

-- The JSON columns are ordered like output_paths. The names, outputs and
-- clock are kept with the session so history reads the same after the source
-- or preset is renamed or removed (or lives on another controller).
CREATE TABLE recording_sessions (
    id             TEXT PRIMARY KEY,
    source_id      TEXT NOT NULL,
    preset_id      TEXT NOT NULL,
    started_at     TEXT NOT NULL,
    stopped_at     TEXT,
    output_paths   TEXT NOT NULL DEFAULT '[]', -- JSON array of file paths, ordered by preset_outputs.sort_order
    status         TEXT NOT NULL,
    error_message  TEXT,
    dropped_frames TEXT NOT NULL DEFAULT '[]', -- frames each leg's encoder couldn't keep up with
    files          TEXT NOT NULL DEFAULT '[]', -- every file each leg wrote (splits write several)
    source_name    TEXT,
    preset_name    TEXT,
    outputs        TEXT NOT NULL DEFAULT '[]', -- `SessionOutputDto` per leg
    clock          TEXT                        -- `SessionClockDto`
);

-- History is listed newest first and paged by start time.
CREATE INDEX recording_sessions_started_at ON recording_sessions (started_at);

-- Sources configured on a node (as opposed to discovered, like NDI) share one
-- table: the config is a JSON object tagged with its `type`.
CREATE TABLE configured_sources (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    config     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT INTO configured_sources (id, name, config, created_at) VALUES
    ('test-1', 'Test Source 1',
     '{"type":"test","pattern":"smpte","width":1920,"height":1080,"fps_num":30,"fps_den":1,"audio_signal":"tone","frequency":440.0,"channels":2}',
     datetime('now')),
    ('test-2', 'Test Source 2',
     '{"type":"test","pattern":"ball","width":1920,"height":1080,"fps_num":30,"fps_den":1,"audio_signal":"pink-noise","frequency":0.0,"channels":2}',
     datetime('now'));

-- Benchmark runs, each stored whole as JSON (BenchmarkRunDto), with the
-- columns needed to list and recover them.
CREATE TABLE benchmark_results (
    id         TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    status     TEXT NOT NULL,
    run        TEXT NOT NULL
);

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

-- Sources that keep a replay buffer, and how. A source that's gone keeps
-- its row (an NDI source can come back); deleting a configured source
-- removes it.
CREATE TABLE replay_buffers (
    source_id TEXT PRIMARY KEY,
    codec     TEXT NOT NULL,
    minutes   INTEGER NOT NULL
);

-- Replay clips saved together on several angles (Shift+S): one in → out,
-- a clip per angle. A group goes when its last clip does.
CREATE TABLE replay_clip_groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

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
    created_at    TEXT NOT NULL,
    group_id      TEXT REFERENCES replay_clip_groups (id) ON DELETE CASCADE
);

-- The NDI senders a node connects to when it's set to connect only to
-- chosen ones (node_config `ndi_connect` = 'chosen'), by NDI name. A sender
-- that's gone keeps its row: it can come back.
CREATE TABLE ndi_subscriptions (
    ndi_name TEXT PRIMARY KEY
);
