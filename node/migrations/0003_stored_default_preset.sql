-- The default preset becomes a stored row that can be edited, deleted or
-- replaced as the default. It keeps the id the built-in one had, so
-- schedules and recording sessions that named it still find it.
INSERT INTO presets (id, name, created_at, updated_at)
VALUES ('default', 'H.264',
        strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

INSERT INTO preset_outputs
    (id, preset_id, name, codec, container, chroma, path_template, sort_order)
VALUES ('default-0', 'default', 'Output', 'h264', 'mov', '420',
        '~/capture-room/{date}/{source_name}_{datetime}.{ext}', 0);

INSERT INTO node_config (key, value) VALUES ('default_preset_id', 'default');

-- Names become unique, ignoring case. The oldest of each name keeps it (the
-- seeded row above is the newest, so a user's own "H.264" wins); the others
-- get " (2)", " (3)", …
-- Ranked into a table first: an UPDATE whose subquery reads the rows it's
-- renaming would see its own earlier renames.
CREATE TEMP TABLE preset_rank AS
SELECT id, (
    SELECT count(*) + 1 FROM presets AS o
    WHERE lower(o.name) = lower(p.name)
      AND (o.created_at < p.created_at OR (o.created_at = p.created_at AND o.id < p.id))
) AS n
FROM presets AS p;

UPDATE presets
SET name = name || ' (' || (SELECT n FROM preset_rank WHERE preset_rank.id = presets.id) || ')'
WHERE (SELECT n FROM preset_rank WHERE preset_rank.id = presets.id) > 1;

-- A suffix can itself match a name that was already there ("A (2)"): the
-- renamed row then takes a piece of its id instead, which is unique.
UPDATE presets
SET name = substr(name, 1, length(name) - length(' (' || (SELECT n FROM preset_rank WHERE preset_rank.id = presets.id) || ')'))
    || ' (' || substr(id, 1, 8) || ')'
WHERE (SELECT n FROM preset_rank WHERE preset_rank.id = presets.id) > 1
  AND EXISTS (
    SELECT 1 FROM presets AS o
    WHERE lower(o.name) = lower(presets.name) AND o.id != presets.id
);

DROP TABLE preset_rank;

CREATE UNIQUE INDEX presets_name ON presets (lower(name));
