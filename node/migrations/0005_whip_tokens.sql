-- WHIP sources need a bearer token: give each existing one its own.
UPDATE configured_sources
SET config = json_set(config, '$.token', lower(hex(randomblob(24))))
WHERE json_extract(config, '$.type') = 'whip'
  AND coalesce(json_extract(config, '$.token'), '') = '';
