# Device MCP servers

Pockymoe can hand MCP servers to ACP harnesses in `session/new`, `session/load`,
`session/resume` and fork. Harnesses that read their own MCP configuration
(Codex, Claude Code) keep using it; this registry is for harnesses that only get
MCP servers through ACP, such as DeepSeek Harness, or for a server you want in
every ACP agent.

```bash
pockymoe mcp list
pockymoe mcp set '{"name":"files","command":"files-mcp","args":["--root","/srv"],"agents":["deepseek"]}'
pockymoe mcp set - < server.json
pockymoe mcp remove files
```

| Field | Meaning |
| --- | --- |
| `name` | 1–64 letters, digits, `-`, `_`; unique |
| `transport` | `stdio` (default), `http` or `sse` |
| `command`, `args`, `env` | stdio only |
| `url`, `headers` | http/sse only; `https`, or `http` on loopback |
| `agents` | ACP agent ids (`deepseek`, `grok`, …); empty means every ACP agent |
| `enabled` | default `true` |

A stdio server is always passed. An HTTP or SSE server is passed only to agents
whose `initialize` advertises `mcpCapabilities.http` / `.sse`. Changes apply to
sessions started or loaded afterwards; restart a thread's harness to pick them
up in a running conversation.

The registry is `<database>.mcp-servers.json`, mode 0600. `env` and `headers`
values can be credentials: listings show them as `<redacted>`. Only the
machine's own CLI credential can list or change it; a thread's agent or a peer
device cannot. Servers that need an OAuth login need a token obtained outside
Pockymoe, passed as a header.
