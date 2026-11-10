Agents can call model APIs through your saved upstream connections, without
seeing their keys.

- **Agent access to connections**: in Settings > Upstreams, a connection's menu has
  **Agents can call it**. It is off for every connection until you turn it on.
  The choice belongs to the device and is not synchronized.
- **For agents**: `pockymoe model list` shows the open connections, their models
  and a base URL per SDK. `pockymoe model ask` returns one answer; OpenAI,
  Anthropic and Gemini clients work against the base URL with the agent's own
  `POCKYMOE_TOKEN` as the key, including streaming. `pockymoe guide models` has
  examples.
- **What stays protected**: the key never leaves the Supervisor. Only inference
  and model-listing routes are reachable, and requests from other devices, through
  the Relay or from isolated environments are refused.
- **Asking without a system prompt**: a completion through a saved connection no
  longer sends an empty system prompt, which some upstreams rejected.
