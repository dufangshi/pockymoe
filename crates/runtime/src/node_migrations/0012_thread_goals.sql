CREATE TABLE IF NOT EXISTS thread_goals (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  codex_thread_id TEXT NOT NULL,
  objective TEXT NOT NULL,
  status TEXT NOT NULL,
  token_budget INTEGER,
  tokens_used INTEGER NOT NULL DEFAULT 0,
  time_used_seconds INTEGER NOT NULL DEFAULT 0,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS thread_goals_thread_updated_idx
  ON thread_goals(thread_id, updated_at);
