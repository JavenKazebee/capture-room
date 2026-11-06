-- The Record page's layout, kept by the controller for every node it has:
-- the order sources show in, and their tags. Rows outlive the source, so a
-- node that's offline for a while keeps its place.
CREATE TABLE source_order (
    node_id   TEXT NOT NULL,
    source_id TEXT NOT NULL,
    position  INTEGER NOT NULL,
    PRIMARY KEY (node_id, source_id)
);

CREATE TABLE source_tags (
    node_id   TEXT NOT NULL,
    source_id TEXT NOT NULL,
    tag       TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (node_id, source_id, tag)
);
