Git working copies and Dokploy execution environments now live beside the
conversation, with an agent-first request and approval flow for moving a thread
into an isolated worker.

- **Working copies**: agents can create, select and inspect worktrees through
  `pockymoe worktree`. The thread records its selected working copy separately
  from observed tool directories, so the Git panel can show when an agent works
  elsewhere. Inspect unstaged and staged changes, compare revisions and browse
  commits. GitHub PR status and Git write controls are not included.
- **Docked tools**: Files, Git and isolation share a resizable right sidebar on
  desktop. The conversation stays usable while the inspector is open. Phones
  use a full-width tool view with a return action.
- **Dokploy workers**: the host Supervisor retains device and thread identity,
  approvals and history while the worker runs in a constrained container.
  Project and required Git metadata use verified bind mounts. CPU, memory,
  swap, process and file-descriptor limits are checked on the actual container;
  lifecycle, recovery and preview routing use the same execution target.
- **Agent-first migration**: an agent proposes limits, network access and
  credential grants with `pockymoe environment request`. The owner reviews the
  exact proposal before an idle thread moves into the environment. Approval
  and recovery state survive Supervisor restarts.
- **Optional credentials**: API mode keeps long-lived model keys on the host
  and gives the worker revocable broker access. Owner-approved native Codex
  and Claude file logins, gh configuration and generic credential mounts are
  also supported; their access and refresh behavior are shown for review.
- **Navigation and tour**: device readings sit in the desktop sidebar, isolated
  devices have a clear mark, and the guided tour runs against an in-page sample
  device without creating work on a real one.

Dokploy connections require Linux and verified shared storage on the same
storage node. A connection and compatible harness image must be configured
before creating workers. Remote storage, E2B, OS keyrings/Keychain and SSH
credential agents are not supported. Bind storage has no hard disk quota.
Native subscription access remains subject to the provider's account rules.

Validated with the relevant Rust and Web checks, real Dokploy lifecycle and
recovery runs, a real Claude subscription turn, and focused desktop/phone
sidebar tests. The Web and Relay deploy separately; devices need this runtime
update to use worktree and isolated-execution features.
