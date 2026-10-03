-- Presets are now sent inline with each start command, so nodes no longer
-- keep a synced copy. Schedules will run on the controller.
DROP TABLE IF EXISTS presets_cache;
DROP TABLE IF EXISTS schedules_cache;

-- Nodes registered manually on a controller (mDNS-discovered nodes are not
-- persisted; they re-announce themselves).
CREATE TABLE IF NOT EXISTS nodes (
    id       TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    url      TEXT NOT NULL,
    added_at TEXT NOT NULL
);

-- The old role key ("node" | "aggregator") becomes a boolean toggle.
INSERT OR IGNORE INTO node_config (key, value)
    SELECT 'controller_enabled', CASE WHEN value = 'node' THEN 'false' ELSE 'true' END
    FROM node_config WHERE key = 'role';
DELETE FROM node_config WHERE key = 'role';
