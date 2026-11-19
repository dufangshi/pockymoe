-- Scheduler queues: time-triggered rows by next_run_at, event-triggered rows by
-- cursor. An idle tick is then one indexed probe instead of a scan of every rule.
CREATE INDEX IF NOT EXISTS automations_time_queue ON automations(state, next_run_at);
CREATE INDEX IF NOT EXISTS automations_event_queue ON automations(state, trigger_kind, event_cursor);
CREATE INDEX IF NOT EXISTS command_executions_state ON command_executions(state);
CREATE INDEX IF NOT EXISTS command_executions_cleanup ON command_executions(id) WHERE cleanup_json IS NOT NULL;
-- Closing or deleting any thread wakes the scheduler's availability sweep,
-- whichever code path made the change.
CREATE TABLE IF NOT EXISTS automation_wake (
 id INTEGER PRIMARY KEY CHECK (id = 1), epoch INTEGER NOT NULL
);
INSERT OR IGNORE INTO automation_wake(id, epoch) VALUES (1, 0);
CREATE TRIGGER IF NOT EXISTS automation_wake_thread_closed
 AFTER UPDATE OF closed_at ON threads WHEN NEW.closed_at IS NOT NULL AND OLD.closed_at IS NULL
 BEGIN UPDATE automation_wake SET epoch = epoch + 1 WHERE id = 1; END;
CREATE TRIGGER IF NOT EXISTS automation_wake_thread_deleted
 AFTER DELETE ON threads
 BEGIN UPDATE automation_wake SET epoch = epoch + 1 WHERE id = 1; END;
