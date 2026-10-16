ALTER TABLE threads ADD COLUMN active_turn_collaboration_mode TEXT;

ALTER TABLE thread_pending_steers
  ADD COLUMN delivery TEXT NOT NULL DEFAULT 'steer';

ALTER TABLE thread_pending_steers
  ADD COLUMN turn_config_json TEXT;

CREATE TABLE IF NOT EXISTS thread_prompt_requests (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  client_request_id TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS thread_prompt_requests_thread_client_request_idx
  ON thread_prompt_requests(thread_id, client_request_id);
