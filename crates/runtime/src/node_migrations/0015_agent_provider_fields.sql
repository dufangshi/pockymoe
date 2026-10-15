ALTER TABLE threads
  ADD COLUMN provider TEXT NOT NULL DEFAULT 'codex';

ALTER TABLE threads
  ADD COLUMN provider_session_id TEXT;

ALTER TABLE threads
  ADD COLUMN provider_turn_id TEXT;

UPDATE threads
SET provider = 'codex',
    provider_session_id = codex_thread_id,
    provider_turn_id = codex_turn_id
WHERE provider_session_id IS NULL;
