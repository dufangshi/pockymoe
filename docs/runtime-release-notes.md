Delegate questions and the work they trigger now form a secondary fold inside the original task's steps.

- Registered wakes durably retain their task and question context through queue acceptance. The timeline shows the delegate name and question subject; opening a question loads its full text and execution history without loading every other round or acknowledging the inbox message.
- Physical turns keep their own usage and billing. The task header aggregates known totals, the latest progress or final reply stays outside the fold, and pending human approvals remain accessible.
- Long human prompts default to a six-line text preview with Expand/Show less controls. Attachments remain visible and Copy retains the complete prompt.
- The local Supervisor serves the Web UI with the correct content types and without an iframe sandbox policy.

Validated with targeted Rust projection and queue/history integration tests, 36 UI tests, type checks, and mobile browser checks for nested lazy loading, both disclosure levels, cost totals, compact layout and long-prompt copying. Windows Device Manager is unchanged.
