ALTER TABLE app_users ADD COLUMN training_goals TEXT;
ALTER TABLE app_users ADD COLUMN plan_start_date TEXT;
ALTER TABLE training_advice ADD COLUMN activity_id INTEGER REFERENCES activities(id) ON DELETE SET NULL;
