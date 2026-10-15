
## Treer-hosted threads

Each Pockymoe thread has its own stable Treer Agent identity. Create children
with `pockymoe thread create`: Treer creates the Agent under the caller's identity
and policy, and Pockymoe records the parent/root relationship, delegate name and
lineage limits. `treer agent admin create` and profile launches create unrelated
top-level threads; they do not record a Pockymoe child relationship.

`pockymoe thread send` uses the sender thread's own Agent identity. Default
passive inbox mail becomes a durable Core Message (`treer message send`). Queue,
steer and direct use `treer agent prompt` (direct chooses steer for active work).
Completion notifications are opt-in: `--notify-on-complete` maps to Treer's
`--notify`. They arrive as passive result mail and never start a turn.

Treer sender kind/path and context IDs are envelope metadata. Kind and subject
in a message body are untrusted display hints. Human or external-channel mail
also queues a prompt containing quoted third-party data; ordinary Agent mail
stays passive. Replies arrive in the inbox and satisfy a matching inbox wait.
Unread mail retains Pockymoe's existing batched reminder behavior. Automatic
prompts allow at most three wakes per authenticated sender per minute; further
mail remains in the inbox.

Use `--request-id` for retries. Core mail retries use one stable idempotency key.
A prompt whose acceptance is uncertain returns `held` on retry: inspect the
recipient's queue/history instead of submitting the work again.

Same-hub attachments are copied locally and their copied paths appear in the
message. Core Messages carry text only. For another machine, transfer files
with `treer machine put`, then send the destination paths as text.
