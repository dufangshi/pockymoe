Codex usage now includes the sub-agents a turn starts.

- **Codex turns that delegate were under-counted.** A Codex sub-agent runs as
  its own session, and its tokens never reached the turn's total: a turn showed
  only the main agent's share. With the `ultra` effort, which delegates by
  itself, most of the work was missing from tokens and cost. A turn now counts
  every model request of the main agent and of the sub-agents it starts, at any
  depth, each priced with its own model.
- Turn totals, the cost tooltip, device statistics and model usage all follow.
  Codex turns also report their number of model requests.
- Turns recorded before this version keep their old totals.
