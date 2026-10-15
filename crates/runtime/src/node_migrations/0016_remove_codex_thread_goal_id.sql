CREATE TABLE `thread_goals_new` (
  `id` text PRIMARY KEY NOT NULL,
  `thread_id` text NOT NULL,
  `provider_session_id` text NOT NULL,
  `objective` text NOT NULL,
  `status` text NOT NULL,
  `token_budget` integer,
  `tokens_used` integer DEFAULT 0 NOT NULL,
  `time_used_seconds` integer DEFAULT 0 NOT NULL,
  `started_at` text NOT NULL,
  `completed_at` text,
  `created_at` text NOT NULL,
  `updated_at` text NOT NULL
);

INSERT INTO `thread_goals_new` (
  `id`,
  `thread_id`,
  `provider_session_id`,
  `objective`,
  `status`,
  `token_budget`,
  `tokens_used`,
  `time_used_seconds`,
  `started_at`,
  `completed_at`,
  `created_at`,
  `updated_at`
)
SELECT
  `id`,
  `thread_id`,
  `codex_thread_id`,
  `objective`,
  `status`,
  `token_budget`,
  `tokens_used`,
  `time_used_seconds`,
  `started_at`,
  `completed_at`,
  `created_at`,
  `updated_at`
FROM `thread_goals`;

DROP TABLE `thread_goals`;
ALTER TABLE `thread_goals_new` RENAME TO `thread_goals`;
