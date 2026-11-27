Google Antigravity is a harness, and file edits by agents that use string request ids no longer hang.

**New**

- **Google Antigravity.** Antigravity runs through Google's own ACP server (`agy acp`). Settings installs it with Google's official script on Linux and macOS and updates it with `agy update`; sign in from Settings → Harnesses, which runs `agy acp --login` in a terminal on the device. Its models (Gemini 4, Gemini 3.8 Flash, Claude and others) and thinking level are chosen like any other agent's.

**Fixes**

- **Agent requests with a string id.** An agent that sends its permission request with a string JSON-RPC id, as Antigravity does, had that request dropped, so its first file edit waited forever. Pockymoe now answers such requests with the id the agent sent.

Update a device from Settings → Check/Update.
