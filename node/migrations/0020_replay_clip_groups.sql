-- Replay clips saved together on several angles (Shift+S): one in → out,
-- a clip per angle. A group goes when its last clip does.
CREATE TABLE replay_clip_groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

ALTER TABLE replay_clips ADD COLUMN group_id TEXT
    REFERENCES replay_clip_groups (id) ON DELETE CASCADE;
