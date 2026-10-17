Content Quality can now write a fresh answer for the reader instead of making
small wording changes. It may shorten process logs, reorganize the answer or add
plain explanations and illustrative analogies. Reader preferences take priority
over preset defaults. The original answer remains available; the editor must
still report outcomes, tests and limitations truthfully.

The editor receives the thread's public text context: user questions and delivered
steers, assistant progress explanations and original answers in chronological
order, with one exact target message identified. Tools, command output and private
reasoning are excluded. Context ends at the target's execution boundary, uses the
existing `/api/threads/{id}/summary` extractor, and has no hidden history cap.
Known local connection credentials and recognizable historical API keys are
removed before submission. Long threads therefore use more input tokens.

A Plugins button on each thread lets you inherit the device default or enable,
disable and configure a plugin for that thread. Settings can return to inherited
defaults without changing another conversation. The device switch remains the
master switch. This is a shared plugin convention, including scoped panels,
renderers, actions and automatic hooks. Read-only shares can see plugin status
but cannot read private settings or change them. Older Supervisors show an
update notice instead of failing repeatedly.

Validated with focused runtime, summary, plugin HTTP/grant and relay/protocol
tests, reader-rewrite upstream probes, and shared UI/component and targeted
browser checks. Windows Device Manager is unchanged. Publication does not restart
an existing Supervisor; update that device through Settings to use the new
backend behavior.
