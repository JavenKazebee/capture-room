-- What a session recorded, kept with it so history reads the same after the
-- source or preset is renamed or removed (or lives on another controller).
ALTER TABLE recording_sessions ADD COLUMN source_name TEXT;
ALTER TABLE recording_sessions ADD COLUMN preset_name TEXT;
-- Each output leg's name and format (`SessionOutputDto`), ordered like
-- output_paths. '[]' for sessions recorded before this column existed.
ALTER TABLE recording_sessions ADD COLUMN outputs TEXT NOT NULL DEFAULT '[]';

-- History is listed newest first and paged by start time.
CREATE INDEX IF NOT EXISTS recording_sessions_started_at ON recording_sessions (started_at);

-- The output to preview a recording with when several are browser-playable.
ALTER TABLE preset_outputs ADD COLUMN preview INTEGER NOT NULL DEFAULT 0;
