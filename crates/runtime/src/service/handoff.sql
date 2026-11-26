UPDATE thread_turns SET provider=(SELECT provider FROM threads WHERE id=thread_id), agent_id=(SELECT agent_id FROM threads WHERE id=thread_id) WHERE provider IS NULL;
CREATE TABLE IF NOT EXISTS thread_harness_sessions (
 thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
 provider TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT '', session_id TEXT,
 model TEXT, turns_seen INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL, last_used_at TEXT NOT NULL,
 PRIMARY KEY(thread_id,provider,agent_id)
);
INSERT OR IGNORE INTO thread_harness_sessions SELECT id,provider,COALESCE(agent_id,''),provider_session_id,model,
 (SELECT COALESCE(MAX(ordinal),0) FROM thread_turns WHERE thread_id=threads.id),created_at,updated_at FROM threads;
CREATE TRIGGER IF NOT EXISTS handoff_turn_identity AFTER INSERT ON thread_turns BEGIN
 UPDATE thread_turns SET provider=COALESCE(NEW.provider,(SELECT provider FROM threads WHERE id=NEW.thread_id)),
 agent_id=COALESCE(NEW.agent_id,(SELECT agent_id FROM threads WHERE id=NEW.thread_id)) WHERE id=NEW.id;
END;