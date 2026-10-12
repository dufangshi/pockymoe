---
name: thread-interaction
description: Coordinate peer Pockymoe threads on this device or on another device of the same owner through passive result handoffs, dependency-aware waiting, explicit task dispatch, urgent corrections and file exchange. Also reserve and diagnose private web previews on this device. Use when creating, messaging or collecting work from Pockymoe peers, or preparing a local website for the user to view.
---

# Pockymoe threads and web previews

Threads are peers on this Supervisor, across Codex, Claude Code, ACP Grok, DSH and
other harnesses. Creation does not copy your conversation, and peers in one workspace
share files unless created with `--worktree`. This page is the index and the rules;
`pockymoe guide TOPIC` has the details and examples, and every command's `--help`
has its exact flags. Data commands print JSON: check the exit status and the returned
delivery state.

## What do you want to do?

| Goal | Start with | Details |
| --- | --- | --- |
| Show the user a local web app | `preview create --port P` **before** starting the server; give the user `openUrl` | `guide preview` |
| Delegate work and collect it | `thread create --name N --kind task --subject S --text-file F`, then `thread wait N`, then `thread close N` | `guide delegate` |
| Send a peer a result, question, task or correction | `thread send ID --kind K --subject S` | `guide messaging` |
| Read or wait for your mail | `inbox`, `inbox wait --kind result --kind question`, `inbox ack` | `guide inbox` |
| Split ordered work across delegates | `task add`, `task claim --wait`, `task done` | `guide tasks` |
| Use another device's threads or files | `device list`, `thread ... --device`, `fs` | `guide devices` |
| Read a peer's history, or retry a send safely | `transcript ID`, `--request-id` | `guide transcript` |
| Schedule a prompt or react to a turn ending | `automation create` | `guide automation` |
| See where you are | `thread self`, `thread tree`, `thread status ID` | `guide delegate` |
| Fix connection or credential problems | | `guide connection` |

## Rules

Choosing a delivery for an existing peer:

- Stopping, replacing, reprioritizing or correcting its **active** work: `--delivery steer`
  (or `direct` when its state is uncertain) with `--interrupt-reason` naming the concrete
  harm or wasted work. Never queue a correction: queue waits for its entire turn.
- Distinct work that can wait for the peer's whole current turn: `--delivery queue --kind task`.
- Results, progress, ready inputs, ordinary questions and acknowledgements: the default
  passive inbox. "Important", "urgent" or "the peer is idle" is not a reason to use
  direct, and a report is never relabeled as a task.
- Label every message with `--subject` and `--kind` (`task`, `question`, `result`,
  `status`). Use `question` only when you are blocked, and `--in-reply-to` when answering.
- Read the whole JSON receipt: `queued` means accepted, not executed; `steered` means the
  backend acknowledged, not that the agent complied; `held` needs inspection, not retries.

Collaborating:

- Mail does not immediately wake you. Unread mail can trigger a batched reminder after
  10 minutes if you are idle. To act promptly on a required result,
  stay in your turn and block with `thread wait NAME...` or
  `inbox wait --kind result --kind question`; to end your turn, first register
  `thread wait NAME... --wake` and say what you are waiting for. Never poll `status` in a
  loop or message yourself.
- A settled delegate finished executing, which is not success. Ask for evidence (files,
  commit, exact test command and result) and check it before merging or closing.
- Lineage limits are depth 3 and 20 open threads per root. Close delegates once
  collected; on a refusal, do not repeat the same call. Prefer 3-5 concurrent delegates.
  Give delegates editing the same files their own `--worktree` (it starts from
  **committed** HEAD).
- Read mail at natural checkpoints (when the notice names something, before dependent
  work, after a build or batch), not after every tool call. Ack only what you handled; do
  not reply "received"; never ack an unanswered question to hide it.
- Notify only the peers that depend on a change, one message per usable batch, with long
  evidence kept in files.

Safety:

- Never print, paste or forward tokens or credentials; use the supplied environment.
- Preserve explicitly requested models; never invent a model ID or substitute one silently.
- Publish, deploy or take other external actions only within the user's authorization.
