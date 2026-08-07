-- Tracks the last local calendar date an automatic sync was enqueued for a
-- user, so visiting the site kicks off at most one background sync per day.
ALTER TABLE app_users ADD COLUMN last_auto_sync_date TEXT;
