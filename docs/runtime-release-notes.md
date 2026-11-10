Agents can call model APIs through your saved upstream connections, without
seeing their keys.

- **Agent access to connections**: in Settings > Upstreams, a connection's menu has
  **Agents can call it**. It is off for every connection until you turn it on.
  The choice belongs to the device and is not synchronized.
- **For agents**: `pockymoe model list` shows the open connections, their models,
  a base URL per SDK and a model key. OpenAI (Chat Completions, Responses),
  Anthropic Messages and Gemini clients work against the base URL with that key,
  including streaming; `pockymoe model env` sets the SDK variables. The model
  key opens only the model proxy, so a script may hold it. `pockymoe guide
  models` has examples.
- **What stays protected**: the key never leaves the Supervisor. Only inference
  and model-listing routes are reachable, and requests from other devices, through
  the Relay or from isolated environments are refused.
- **Asking without a system prompt**: a completion through a saved connection no
  longer sends an empty system prompt, which some upstreams rejected.
- **Phone windows above the keyboard** (Web): a window that rises from the bottom
  on a phone, such as Harness settings opened from the model menu, now fits the
  part of the screen the keyboard leaves visible, instead of reaching under the
  keyboard or above the screen.
