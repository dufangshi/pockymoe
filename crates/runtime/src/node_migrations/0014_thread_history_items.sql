CREATE TABLE IF NOT EXISTS thread_history_items (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  item_id TEXT NOT NULL,
  item_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS thread_history_items_thread_turn_item_idx
  ON thread_history_items (thread_id, turn_id, item_id);
