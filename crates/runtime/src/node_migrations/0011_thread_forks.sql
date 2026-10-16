CREATE TABLE IF NOT EXISTS thread_forks (
  id TEXT PRIMARY KEY NOT NULL,
  source_thread_id TEXT NOT NULL,
  source_turn_id TEXT,
  source_turn_index INTEGER,
  forked_thread_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
