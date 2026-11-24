# pockymoe guide inbox

When and how to read, wait or wake on and acknowledge mail; asking with `thread ask`.

## When to read your inbox

Waiting mail is announced at the start of a turn with a bounded digest, prioritizing
questions/tasks over results/status. It is not a complete mailbox and does not enter
an already running turn. Batch reads at natural boundaries:

- **When the notice names something.** It tells you what is waiting; decide then
  whether it changes what you are about to do.
- **Before work that depends on a peer.** A result you never read is a result you do
  not have.
- **After a meaningful build/test/batch finishes.** Mail may have arrived meanwhile;
  do not reread the entire inbox after every small tool call.
- **When you have nothing else to do but expect mail:** block on it with
  `pockymoe inbox wait` rather than ending your turn or looping.

```bash
pockymoe inbox list --kind question --kind task
pockymoe inbox list --from-thread producer --kind result
pockymoe inbox wait --kind result --kind question   # result or an upstream blocker
pockymoe inbox wait --from-thread reviewer --kind result --kind question
pockymoe inbox wait --new                           # deliberately defer old mail
pockymoe inbox wait --in-reply-to MESSAGE_ID        # the answer to your question
pockymoe inbox wait --from-thread DEVICE/THREAD_UUID --kind result --kind question --wake
```

`inbox wait` returns the matching messages with text inline. Unacknowledged mail
satisfies it immediately, so acknowledge what you have handled (or use `--new` / a
filter while deliberately deferring something). Filtering happens before the bounded
page is selected. Preserve the filters when paging with `--before`. Acknowledge only
what you have handled or deliberately recorded. Ack removes mail from unread views;
it neither deletes history nor proves task success. Do not ack an unanswered question
merely to hide it: current thread waits treat its ack as no longer waiting.

When waiting for results, include questions so a producer asking you for missing input
can unblock you. Sender filters must include any peer whose question you must answer.
Do not have two agents wait on each other's result without naming and resolving the
dependency; expose blockers in the task board or a question instead of repeatedly
waking both to ask for status. For a long-lived producer's intermediate batches, wait
for inbox results rather than its whole thread to finish.

### Required handoffs need a wait; unread mail has a delayed fallback

Completion notifications are passive: `--notify-on-complete` files a message without
immediately starting a turn. If new mail remains unread for 10 minutes and the
recipient has no active turn or queued input, the Supervisor queues one batched
reminder. Each batch is reminded only once, even across restarts; further batches
are at least 10 minutes apart. Closed threads are not reopened, and historical
mail from before this feature is not enrolled. Do not rely on this delayed
fallback when a result is required to complete your task.

- **If you must act on the result, stay in your turn** and block on it:
  `thread wait NAME` or `inbox wait`. Do independent work first if you have any.
- **If your turn must end, hand off explicitly** with `thread wait NAME... --wake` for
  your delegates, or `inbox wait ... --wake` for mail (results from another device, a
  reply), and say in your final message what you wait for and that you will resume
  when it lands.

`inbox wait --wake` takes the same filters as the blocking wait and fires once, on
matching mail that arrives after you register it (mail already waiting is yours to
read now). Its report previews up to 5 messages; read, ack and reply as usual. Both
kinds of wake are receiver-owned and one-shot, share a limit of 3 pending, and are
not subscriptions to arbitrary sibling artifacts. Only register one when you need the
continuation.
`pockymoe thread wakes` lists them; `--cancel WAKE_ID` or `--cancel-all` drops ones
you no longer need. A mail wake expires after 24 hours (`--expires-in SECONDS`, up to
7 days) and then leaves a "Mail wake expired" status note in your inbox.

### Asking and waiting for the answer

When you are blocked on a decision only another thread can make, ask instead of
guessing or stopping:

```bash
pockymoe thread ask parent --subject 'Which staging key?' \
  --text 'The task names two keys; which one may I use for the smoke test?' --request-id key-q
```

It mails a `question` and blocks until a reply `--in-reply-to` it arrives (default
600 s, max 1800 s), then prints it in `reply` (marked read; `inbox ack` it once
handled). An idle ancestor gets the question as a turn right away, a busy one at its
next idle moment. On `answered: false` the question stays open: wait again with
`inbox wait --in-reply-to QUESTION_ID` or rerun with the same `--request-id`; do not
ask again. Only one prompt-delivered question to the same thread is open at a time.

Do not work around this by steering the peer, polling `status` in a loop, or sending
yourself direct messages; `wait` is cheaper than all of them.


## Read and acknowledge your inbox

```bash
pockymoe inbox
pockymoe inbox list --limit 10
pockymoe inbox list --kind result --kind question --from-thread producer
pockymoe inbox read MESSAGE_ID
pockymoe inbox ack MESSAGE_ID
pockymoe inbox list --all --limit 10
```

The caller's identity selects the mailbox. `--thread THREAD_ID` explicitly selects another mailbox, including from a shell without managed thread identity. IDs/URLs address the same local Supervisor; attribution is not a per-agent security boundary.

List returns bounded previews (240 characters) and envelope metadata. It defaults to 20 active unread messages, capped at 100, displaying the selected page chronologically. `--kind` and `--from-thread` are repeatable on list/wait. Follow `nextBefore` with `--before MESSAGE_ID`, preserving filters and `--all` if used. `--all` includes acknowledged and superseded history. One page need not contain every unread message.

Read returns at most 8192 Unicode characters. Follow `nextTextOffset` with `inbox read MESSAGE_ID --text-offset OFFSET` when needed. Reading (including inline `inbox wait` results) records `readAt` and suppresses the delayed reminder. List previews do not. Reading does **not** mark mail processed, including after a crash. Acknowledge only messages you have handled or deliberately recorded for later action. `ack` accepts 1–100 explicit IDs atomically and is safe to repeat; acknowledged messages remain accessible with `--all` or `read`. Do not mark messages acknowledged merely to hide a queue count.

A reply uses the sender's `fromThreadId`:

```bash
pockymoe thread send SENDER_ID --kind result --subject 'Requested build result' \
  --in-reply-to MESSAGE_ID --text-file /tmp/result.txt
pockymoe inbox ack MESSAGE_ID
```

A passive reply does not immediately wake an idle sender. The sender should collect
or register its own wake for a required result; do not upgrade results to direct/queue to compensate. Include the
concrete response destination and handoff conditions in delegated instructions.
