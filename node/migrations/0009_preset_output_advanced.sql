-- An output's Advanced settings as JSON (`OutputAdvanced`); '{}' is all defaults.
ALTER TABLE preset_outputs ADD COLUMN advanced TEXT NOT NULL DEFAULT '{}';
