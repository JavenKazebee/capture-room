-- Monthly schedules: on a day of the month, or on the nth (or last) of a
-- weekday, which is the one bit set in `weekdays`. Both NULL otherwise.
ALTER TABLE schedules ADD COLUMN month_day INTEGER;
ALTER TABLE schedules ADD COLUMN month_week INTEGER;
