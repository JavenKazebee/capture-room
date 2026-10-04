-- Video frames each output leg dropped because its encoder couldn't keep up;
-- a JSON array ordered like output_paths.
ALTER TABLE recording_sessions ADD COLUMN dropped_frames TEXT NOT NULL DEFAULT '[]';
