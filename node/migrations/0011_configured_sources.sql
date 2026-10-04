-- Sources configured on a node (as opposed to discovered, like NDI) share one
-- table: the config is a JSON object tagged with its `type`.
CREATE TABLE IF NOT EXISTS configured_sources (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    config     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT OR IGNORE INTO configured_sources (id, name, config, created_at)
    SELECT id, name,
           json_object('type', 'test', 'pattern', pattern, 'width', width, 'height', height,
                       'fps_num', fps_num, 'fps_den', fps_den, 'audio_signal', audio_signal,
                       'frequency', frequency, 'channels', channels),
           created_at
    FROM test_sources;

DROP TABLE test_sources;
