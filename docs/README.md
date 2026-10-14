# Documentation

Maintained documentation for Pockymoe. For the product overview, start with the
[README](../README.md).

## Index

**Setup**

- [Self-hosting a relay](self-host-relay.md) ([简体中文](self-host-relay.zh-CN.md))
- [Device setup, harnesses and upstreams](device-setup.md)
- [Native installation and GitHub runtime releases](github-runtime.md)
- [Treer integration](treer-integration.md): hub, attach, pinned installation and launch profiles

**Development**

- [Development](development.md): layout, running locally, tests, the update API
- [Architecture](architecture.md) and [code structure](code-structure.md)
- [CI scope](ci.md)
- [Web i18n](i18n.md)
- [Theme presets](theme-presets.md) and [theme packs](theme-packs.md)
- [Web live updates and request budget](web-live-updates.md)
- [Rename: Remote Codex → Pockymoe](rename-pockymoe.md): identifiers that must keep their old names

**Operations and reliability**

- [Supervisor updates and interrupted-task recovery](supervisor-update-recovery.md)
- [Execution state, delivery and Supervisor ownership](execution-reliability.md)
- [Auth and connectivity modes](auth-and-connectivity-modes.md)
- [Relay account security and encrypted transport](relay-security-operations.zh.md)
- [Account key trust](account-key-trust.md): device binding, strict mode, signed sharing, static Web hosting, migration
- [Device web previews through the relay](port-previews.md)

**Features and interfaces**

- [Thread interaction CLI](thread-interaction.md) and [cross-device peers](cross-device-peer.zh.md)
- [Thread event and item schema](thread-event-schema.md)
- [Automations and hooks](unified-hooks.md) and [watch summaries](watch-statistics.md)
- [Conversation search](global-search.md)
- [Thread notifications](thread-notifications.md)
- [Device monitoring](device-monitor.md)
- [Model prices and subscription usage](model-pricing-and-subscription-usage.md) and [token speed](turn-token-speed.md)
- [Native subagents](native-subagents.md)
- [Chat diagrams](mermaid-diagrams.md)
- [Message shortcuts](account-composer-shortcuts.md)
- [ACP slash commands, forks and Grok usage](acp-slash-fork-and-usage.md)

**Harnesses**

- [Harness lifecycle](harness-lifecycle.md): when harness processes load and are released, limits, memory
- [Managed ACP adapters](managed-acp-adapters.md)
- [DeepSeek Harness bridge](dsh-adapter.md)
- Claude: [background waits and wakes](claude-background-wake.md), [scheduled-turn history](claude-scheduled-history.md)

**Release input**

- [runtime-release-notes.md](runtime-release-notes.md) is the body of the next
  GitHub runtime release (`runtime-release.yml` reads it). Replace it for each
  release; published notes live on GitHub Releases.

## Rules for this directory

These rules apply Anthropic's guidance for agent instruction files: keep
always-loaded instructions short, link to docs instead of copying them, and
prune what is stale ([best practices](https://code.claude.com/docs/en/best-practices),
[CLAUDE.md and AGENTS.md](https://code.claude.com/docs/en/memory)).

**What belongs here.** Documents that stay true for the current code and that a
new contributor or coding agent needs:

- how to set up, operate, release and recover the system;
- architecture, conventions and compatibility constraints;
- behavior and contracts of features, APIs and the CLI;
- how to test a component when that is not obvious.

**What does not belong here.** Put it in [`.scratch/`](../.scratch/README.md),
which git ignores, or leave it out of the repository:

- plans, proposals, design discussions and task lists;
- progress notes, verification or test-run records, audits and screenshots
  taken while working;
- incident timelines and operations logs;
- per-version release notes (GitHub Releases holds them);
- personal paths, host-specific details that only matter once, and secrets.

When a plan ships or an incident is resolved, write the lasting outcome into the
document for that component, in the present tense, and delete the rest.

**Writing and maintaining.**

- Describe current behavior for a reader who does not know the history. Leave
  out branch names, "this round" and "not yet released".
- One topic per file, with a kebab-case name and no dates, versions or words
  like "plan" or "preview". Write in English; a user-facing guide may have a
  `.zh-CN.md` translation.
- Update or delete a document in the same commit as the change that affects it.
  When renaming or removing one, search the repository for references to it;
  code comments and skills link to some documents.
- Add every new document to the index above. Keep images under
  `docs/assets/<topic>/`, and only those a document uses.

**Agent instructions.** `AGENTS.md` (also loaded as `CLAUDE.md`) holds only
rules that every session needs, ideally well under 200 lines. Multi-step
procedures belong in skills under `.agents/skills/`, and detail belongs in a
document linked from `AGENTS.md`. For each line, ask whether removing it would
cause mistakes; if not, remove it.
