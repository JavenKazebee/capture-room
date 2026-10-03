-- Preset outputs now reference their preset and are deleted with it. SQLite
-- can't add a foreign key to an existing table, so it is rebuilt; outputs whose
-- preset is already gone are dropped.
CREATE TABLE preset_outputs_new (
    id            TEXT PRIMARY KEY,
    preset_id     TEXT NOT NULL REFERENCES presets(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    codec         TEXT NOT NULL,
    container     TEXT NOT NULL,
    resolution    TEXT,               -- "1920x1080"; null = match source
    framerate     TEXT,               -- "30" or "30000/1001"; null = match source
    bitrate_kbps  INTEGER,            -- null = encoder default
    chroma        TEXT NOT NULL DEFAULT '420',
    path_template TEXT NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0
);

INSERT INTO preset_outputs_new
    (id, preset_id, name, codec, container, resolution, framerate, bitrate_kbps, chroma, path_template, sort_order)
SELECT id, preset_id, name, codec, container,
       NULLIF(trim(resolution), ''), NULLIF(trim(framerate), ''),
       bitrate_kbps, chroma, path_template, sort_order
FROM preset_outputs
WHERE preset_id IN (SELECT id FROM presets);

DROP TABLE preset_outputs;
ALTER TABLE preset_outputs_new RENAME TO preset_outputs;
CREATE INDEX preset_outputs_preset_id ON preset_outputs (preset_id);
