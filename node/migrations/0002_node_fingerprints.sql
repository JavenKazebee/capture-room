-- Nodes found over mDNS are stored too once approved, and every node with the
-- SHA-256 of its TLS key (NULL for one added before TLS, until it's pinned).
ALTER TABLE nodes ADD COLUMN fingerprint TEXT;
