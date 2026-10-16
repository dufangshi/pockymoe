# Thread event and item schema

The Rust wire contract is in `crates/protocol/src/lib.rs` and
`crates/protocol/src/thread_schema.rs`. The Web contract is in
`packages/shared/src/index.ts`. JSON field names are camelCase. The enums encode
the existing strings; they do not add wrapper objects or change payload shapes.

## Events

`ThreadEventEnvelope` contains `type`, `threadId`, `timestamp`, and `payload`.
`type` is a `ThreadEventType`; `payload` remains arbitrary JSON. The Supervisor
socket forwards these envelopes from the runtime event bus. Events may be
partial updates: clients should merge them or refresh the relevant HTTP state.

| Type | Payload / meaning |
| --- | --- |
| `thread.updated` | Partial thread fields, pending steers or a reason to refresh |
| `thread.context.updated` | `contextUsage` |
| `thread.subagents.updated` | `turnId`, `activeSubagents` |
| `thread.goal.updated` | `goal`, `goalHistory`, optional `turnId` |
| `thread.goal.cleared` | `goalHistory` |
| `thread.turn.token.updated` | `turnId`, usage/pricing and optional model/reasoning metadata |
| `thread.turn.started` | `turnId` |
| `thread.item.started` | `turnId`, `item`; includes updates to a running item |
| `thread.item.completed` | `turnId`, `item`; terminal item update |
| `thread.plan.updated` | `turnId`, `explanation`, `plan` |
| `thread.request.created` | `request` |
| `thread.request.resolved` | `requestId` |
| `thread.output.delta` | `turnId`, `itemId`, `sequence`, `delta`; persisted output may also include `text`, `createdAt` |
| `thread.turn.completed` | `turnId`, `status`, `error` |
| `thread.persistence.failed` | `message`; failed durable write notification |
| `thread.turn.failed` | `turnId`, `error`, optional `willRetry` |
| `thread.harness.ready` | `turnId`, `instanceId`; writer selected for this turn |
| `thread.collaboration.updated` | `turnId`, `collaborationMode` |
| `thread.context.delivered` | `providerSessionId`; collaboration context delivered |
| `runtime.usage.updated` | `turnId`, provider `usage`; normalized by the service |
| `device.ports.changed` | `reason`; device notification with an empty `threadId` |

The goal events and `thread.turn.failed` are declared compatibility values;
current Rust producers use `thread.updated` for goal refreshes and
`thread.turn.completed` with a terminal status for failed turns.
`runtime.usage.updated` and `thread.context.delivered` also serve internal
service consumers. The type list is not a promise that every event is emitted
on every harness or turn.

The socket additionally produces `supervisor.connected`, `supervisor.pong`,
and `shell.*` envelopes. Those have separate contracts and are not
`ThreadEventEnvelope` discriminants.

## History items

`ThreadHistoryItemDto.kind` is a `ThreadItemKind`. Core fields are `id`, `kind`,
and `text`; optional fields include `createdAt`, `previewText`, `detailText`,
`status`, `sequence`, `sourceTurnId`, and `artifact`. Rust retains additional
top-level fields in the flattened `extra` map, including provider metadata,
background-wait information, file-change details and deferred-detail markers.

The optional Content Quality plugin attaches `polishedReply` to a completed
`agentMessage`: `{text, preset, language, model, sourceHash, createdAt,
requestHash}`. The original `text` remains authoritative. `sourceHash` is the
SHA-256 of its UTF-8 bytes; consumers show the derivative only while this hash
matches. A saved derivative emits `thread.item.completed` for the same item.
This presentation field does not create another agent turn or alter provider
history. Consumers without the enabled plugin continue to display the original.

| Kind | Meaning |
| --- | --- |
| `userMessage` | User prompt |
| `agentMessage` | Assistant output |
| `reasoning` | Assistant reasoning segment |
| `plan` | Plan steps |
| `contextCompaction` | Context summary / compaction |
| `commandExecution` | Shell command and output |
| `webSearch` | Web search or fetch |
| `fileRead` | File read |
| `fileChange` | File edit, deletion or move |
| `agentToolCall` | Native subagent tool activity |
| `toolCall` | General tool activity |
| `generic` | Native background wait / task notice |
| `imageView` | Image-view record imported from Codex history |
| `other` | Normalized provider record without a more specific mapping |
| `artifact` | Shared UI artifact contract |
| `image` | Shared UI image contract |
| `hook` | Shared UI hook contract |
| `skillToolCall` | Shared UI skill invocation contract |

Item `status` remains an optional open string, distinct from turn status.
Current records include `running`, `inProgress`, `waiting`, `completed`,
`failed`, and `interrupted`; imported provider phases can also appear here.
Do not interpret every item status as a turn lifecycle value.

## Thread and turn status

`ThreadDto.status` uses `ThreadStatus` (`ThreadStatusDto` in TS).

| Status | Meaning |
| --- | --- |
| `idle` | No active turn |
| `running` | Active turn |
| `recovering` | Backend state unconfirmed; waiting for reconnection |
| `interrupted` | Turn interrupted |
| `failed` | Turn failed |
| `not_loaded` | Declared compatibility value for an unloaded thread |
| `system_error` | Declared compatibility value for a system error |

`ThreadTurnDto.status` and fork-turn option status use `TurnStatus`
(`TurnStatusDto` in TS): `inProgress`, `recovering`, `completed`,
`interrupted`, `failed`. A completed turn normally returns its thread to
`idle`. The thread and turn vocabularies are different.

## Compatibility and contract checks

Kinds, statuses and event types may grow additively. Every Rust discriminant
has `Unknown(String)`: deserialization preserves an unrecognized string and
serialization writes the exact same string back. This differs from the known
`other` item kind, which a provider mapper can choose deliberately.
Unknown item fields and event payloads also survive protocol round trips.

TS unions describe the known values, not runtime validation of incoming JSON.
Consumers at an untrusted or version-skewed boundary must tolerate unknown
kinds with a generic text/item fallback, ignore unsupported events, and retain
unknown statuses without treating them as a successful completion. Mappers
should explicitly handle all known variants and provide an unknown fallback.

When adding a value, update the corresponding mapping in
`crates/protocol/src/thread_schema.rs`, the TS literal union or
`ThreadEventPayloadMap`, producers/consumers, and this document together.
`KNOWN_VALUES` comes from the same mapping as serialization, so no separate
Rust inventory needs updating. Keep the TS aliases as literal unions; the
contract parser rejects widening them to `string`.

Run:

```sh
cargo test -p pockymoe-protocol --locked
pnpm --filter @pockymoe/shared typecheck
pnpm --filter @pockymoe/shared test
```

The shared unit-test entry point invokes the same Rust contract test, requiring
Cargo and avoiding a duplicated fixture. The Rust test includes the TS source
at compile time and compares all four known-value sets, including duplicates.
Wire tests check known/unknown string round trips and flattened payload
preservation. These checks cover discriminants; they do not validate arbitrary
payload shapes or the separate shared copy in the external thread UI repo.
PR checks run the protocol tests when the protocol or shared package changes,
including changes confined to TS declarations.
