CREATE TABLE IF NOT EXISTS thread_pending_steers (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  client_request_id TEXT,
  display_prompt TEXT NOT NULL,
  submitted_prompt TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS thread_pending_steers_thread_created_idx
  ON thread_pending_steers(thread_id, created_at);
