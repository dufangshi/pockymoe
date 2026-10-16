ALTER TABLE threads ADD COLUMN reasoning_effort TEXT;
ALTER TABLE threads ADD COLUMN collaboration_mode TEXT NOT NULL DEFAULT 'default';
