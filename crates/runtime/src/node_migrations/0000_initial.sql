CREATE TABLE IF NOT EXISTS hosts (
  id TEXT PRIMARY KEY NOT NULL,
  hostname TEXT NOT NULL,
  platform TEXT NOT NULL,
  tailscale_name TEXT,
  created_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workspaces (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL,
  label TEXT NOT NULL,
  abs_path TEXT NOT NULL UNIQUE,
  is_favorite INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  last_opened_at TEXT
);

CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL,
  codex_thread_id TEXT,
  title TEXT NOT NULL,
  model TEXT,
  approval_mode TEXT,
  status TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_turn_started_at TEXT,
  last_turn_completed_at TEXT,
  last_viewed_at TEXT,
  is_pinned INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS shell_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL,
  thread_id TEXT,
  tmux_session_name TEXT,
  cwd TEXT NOT NULL,
  status TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_activity_at TEXT
);

CREATE TABLE IF NOT EXISTS viewer_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT,
  shell_id TEXT,
  connected_at TEXT NOT NULL,
  last_heartbeat_at TEXT,
  active_tab TEXT
);

CREATE TABLE IF NOT EXISTS notifications (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  is_read INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS policies (
  id TEXT PRIMARY KEY NOT NULL,
  key TEXT NOT NULL UNIQUE,
  value_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
