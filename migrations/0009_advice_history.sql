-- Training overviews become a per-day history the UI can browse with a calendar
-- picker: at most one overview row per local date, with its follow-up chat
-- stored alongside it.
ALTER TABLE training_advice ADD COLUMN advice_date TEXT;

UPDATE training_advice
SET advice_date = date(created_at)
WHERE advice_date IS NULL;

-- Collapse any pre-existing same-day overviews down to the newest one so the
-- unique index below can be created.
DELETE FROM training_advice
WHERE activity_id IS NULL
  AND id NOT IN (
      SELECT MAX(id)
      FROM training_advice
      WHERE activity_id IS NULL
      GROUP BY advice_date
  );

CREATE UNIQUE INDEX idx_training_advice_overview_date
    ON training_advice(advice_date)
    WHERE activity_id IS NULL;

CREATE TABLE advice_chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    advice_id INTEGER NOT NULL REFERENCES training_advice(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_advice_chat_messages_advice ON advice_chat_messages(advice_id, id);
