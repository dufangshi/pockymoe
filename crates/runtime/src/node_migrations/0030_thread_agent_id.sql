ALTER TABLE threads ADD COLUMN agent_id TEXT;

UPDATE threads
SET agent_id = model,
    model = NULL
WHERE provider = 'acp'
  AND agent_id IS NULL
  AND model IS NOT NULL
  AND provider_session_id LIKE model || '::%';
