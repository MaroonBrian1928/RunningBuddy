ALTER TABLE activities ADD COLUMN start_date_local TEXT;

-- Backfill from the raw Strava payload, which carries the athlete's wall-clock time.
UPDATE activities
SET start_date_local = json_extract(raw_activity_json, '$.start_date_local')
WHERE start_date_local IS NULL;
