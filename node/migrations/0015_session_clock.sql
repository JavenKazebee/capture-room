-- Where a recording sits on its clock (`SessionClockDto`, JSON): the clock
-- domain, the synchronized start it waited for, and when its first frame was
-- due. NULL for older sessions.
ALTER TABLE recording_sessions ADD COLUMN clock TEXT;
