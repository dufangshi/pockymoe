Agent threads coordinate more reliably, and a thread recovers when its harness process dies.

**Fixes**

- **A harness that died while idle.** When a Codex, Claude or other harness process crashed or was killed while its thread was idle, every later turn in that thread failed (`Broken pipe`, `Codex fork bridge closed`) until the Supervisor restarted. The next turn now starts a new process and resumes the session.
- **A delegated task that never ran.** `pockymoe thread send --kind task` used to mail the task unless `--delivery queue` was given, so the delegate never started it. A task is now queued by default; pass `--delivery inbox` to only mail it.

**Agent-thread coordination**

- **A delegate that cannot start is reported.** When a delegate's queued work keeps failing to start (for about 30 seconds), `thread wait` returns and a registered wake fires once, with state `stalled` and the harness's own error. Before, the caller waited without a reason.
- **How a turn ended.** `thread wait`, wake reports, `thread tree` and `thread status` carry an `outcome`: completed, failed with the error, interrupted and by whom, stalled, or blocked.
- **Asking in one command.** `pockymoe thread ask parent --subject ... --text ...` mails a question and waits for the answer. An idle parent gets the question as a turn at once; a parent that registered a wake reads it in that wake, with the command that answers it.
- **Wakes on mail, including from other devices.** `pockymoe inbox wait --kind result --from-thread DEVICE/THREAD --wake` continues the caller when matching mail arrives, so work sent to another device no longer needs polling. A mail wake expires (24 hours by default, `--expires-in`), and `pockymoe thread wakes` lists and cancels pending wakes.
- **A delegate knows the rules.** The first task a thread receives from its parent carries a short contract: who it works for, that its final message is reported automatically, where evidence goes, to ask when blocked, and not to delegate further unless asked.
- **No endless retries.** After three failed turns in a row, a thread's further tasks to that delegate are refused with the last error; `--retry-failed` overrides. The owner is never refused.
- **Fewer wasted turns.** A question already quoted in a wake does not trigger the 10-minute reminder; a completion notice that repeats the delegate's own result within a minute starts no wake; a wake on delegates that have all finished returns their results at once instead of waking later.

**Interface**

- The bell above the composer also lists mail wakes ("Waiting for mail from ..."), and a delegate that cannot start reads "Cannot start".

Update a device from Settings → Check/Update.
