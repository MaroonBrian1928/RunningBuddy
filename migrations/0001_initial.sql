CREATE TABLE app_users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE app_sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES app_users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL
);

CREATE TABLE athletes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    strava_athlete_id INTEGER NOT NULL UNIQUE,
    username TEXT,
    firstname TEXT,
    lastname TEXT,
    profile_url TEXT,
    raw_profile_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE strava_tokens (
    athlete_id INTEGER PRIMARY KEY REFERENCES athletes(id) ON DELETE CASCADE,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    scopes TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE webhook_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    object_type TEXT NOT NULL,
    object_id INTEGER NOT NULL,
    aspect_type TEXT NOT NULL,
    owner_id INTEGER NOT NULL,
    subscription_id INTEGER,
    updates_json TEXT NOT NULL DEFAULT '{}',
    received_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    processed_at TEXT,
    error TEXT
);

CREATE TABLE sync_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    payload_json TEXT NOT NULL DEFAULT '{}',
    attempts INTEGER NOT NULL DEFAULT 0,
    run_after TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_error TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE activities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    strava_activity_id INTEGER NOT NULL UNIQUE,
    athlete_id INTEGER REFERENCES athletes(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    sport_type TEXT,
    start_date TEXT,
    elapsed_time_seconds INTEGER,
    moving_time_seconds INTEGER,
    distance_meters REAL,
    total_elevation_gain REAL,
    average_heartrate REAL,
    max_heartrate REAL,
    average_speed REAL,
    max_speed REAL,
    average_cadence REAL,
    average_watts REAL,
    kilojoules REAL,
    suffer_score REAL,
    visibility TEXT,
    deleted_at TEXT,
    private_unavailable INTEGER NOT NULL DEFAULT 0,
    raw_activity_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE activity_streams (
    activity_id INTEGER PRIMARY KEY REFERENCES activities(id) ON DELETE CASCADE,
    time_json TEXT,
    distance_json TEXT,
    heartrate_json TEXT,
    cadence_json TEXT,
    velocity_smooth_json TEXT,
    watts_json TEXT,
    altitude_json TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE training_advice (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_window_days INTEGER NOT NULL,
    summary TEXT NOT NULL,
    load_observations_json TEXT NOT NULL,
    risks_json TEXT NOT NULL,
    next_7_days_json TEXT NOT NULL,
    recovery_notes TEXT NOT NULL,
    confidence REAL NOT NULL,
    safety_note TEXT NOT NULL,
    raw_response_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_activities_start_date ON activities(start_date);
CREATE INDEX idx_webhook_events_processed ON webhook_events(processed_at);
CREATE INDEX idx_sync_jobs_status_run_after ON sync_jobs(status, run_after);
