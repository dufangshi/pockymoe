ALTER TABLE threads
  ADD COLUMN fast_mode INTEGER NOT NULL DEFAULT 0;

ALTER TABLE threads
  ADD COLUMN fast_base_model TEXT;

ALTER TABLE threads
  ADD COLUMN fast_base_reasoning_effort TEXT;

CREATE TABLE IF NOT EXISTS thread_activity_notes (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS thread_activity_notes_thread_created_idx
  ON thread_activity_notes(thread_id, created_at);
