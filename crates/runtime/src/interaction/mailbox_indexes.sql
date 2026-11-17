-- SQLite maintains these for every delivery, read, acknowledgement and deletion,
-- including host mail and existing records; no separate backfill state is needed.
CREATE INDEX IF NOT EXISTS mailbox_open_idx ON kv(
    json_extract(value,'$.fromThreadId'), json_extract(value,'$.kind'),
    json_extract(value,'$.id'), key
) WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.acknowledgedAt') IS NULL;
CREATE INDEX IF NOT EXISTS mailbox_reply_idx ON kv(
    json_extract(value,'$.threadId'), json_extract(value,'$.inReplyTo')
) WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.inReplyTo') IS NOT NULL;
CREATE INDEX IF NOT EXISTS mailbox_ack_age_idx ON kv(julianday(json_extract(value,'$.acknowledgedAt')))
WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.acknowledgedAt') IS NOT NULL;
