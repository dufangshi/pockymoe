CREATE TABLE IF NOT EXISTS treer_bindings (
    thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_instance_id TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS treer_submissions (
    id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    cancelled INTEGER NOT NULL DEFAULT 0,
    notified INTEGER NOT NULL DEFAULT 0,
    notification_attempts INTEGER NOT NULL DEFAULT 0,
    notification_next_attempt INTEGER NOT NULL DEFAULT 0,
    UNIQUE(thread_id, operation_id)
);

CREATE TABLE IF NOT EXISTS treer_hub_identity (id INTEGER PRIMARY KEY CHECK(id=1), hub_id TEXT NOT NULL);
