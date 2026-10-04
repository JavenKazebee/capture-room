-- Every file each output leg wrote (split recordings write several); a JSON
-- array of arrays ordered like output_paths.
ALTER TABLE recording_sessions ADD COLUMN files TEXT NOT NULL DEFAULT '[]';
