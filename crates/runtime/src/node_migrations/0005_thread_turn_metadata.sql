CREATE TABLE IF NOT EXISTS thread_turn_metadata (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  model TEXT,
  reasoning_effort TEXT,
  reasoning_effort_available INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS thread_turn_metadata_thread_turn_idx
  ON thread_turn_metadata(thread_id, turn_id);
