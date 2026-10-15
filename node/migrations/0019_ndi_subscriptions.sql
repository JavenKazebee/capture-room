-- The NDI senders a node connects to when it's set to connect only to
-- chosen ones (node_config `ndi_connect` = 'chosen'), by NDI name. A sender
-- that's gone keeps its row: it can come back.
CREATE TABLE ndi_subscriptions (
    ndi_name TEXT PRIMARY KEY
);
