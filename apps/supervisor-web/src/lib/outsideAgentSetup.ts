export type SetupCommand = [app: string, command: string];
/** Each command was run as written against a lab Supervisor (docs/mcp-outside-agents.md). */
export function setupCommands(endpoint?: string): { local: SetupCommand[]; http: SetupCommand[] } {
  const local: SetupCommand[] = [
    ['Claude Code', 'claude mcp add --scope user pockymoe -- pockymoe mcp serve'],
    ['Codex', 'codex mcp add pockymoe -- pockymoe mcp serve'],
    ['Gemini CLI', 'gemini mcp add --scope user pockymoe pockymoe mcp serve'],
    ['Cursor · ~/.cursor/mcp.json', JSON.stringify({ mcpServers: { pockymoe: { command: 'pockymoe', args: ['mcp', 'serve'] } } })],
    ['MCP Inspector', 'npx -y @modelcontextprotocol/inspector --cli pockymoe mcp serve --method tools/list'],
  ];
  const http: SetupCommand[] = endpoint ? [
    ['Claude Code', `claude mcp add --scope user --transport http pockymoe-http ${endpoint}\nclaude mcp login pockymoe-http`],
    ['Codex', `codex mcp add pockymoe-http --url ${endpoint}`],
    ['Cursor · ~/.cursor/mcp.json', `${JSON.stringify({ mcpServers: { 'pockymoe-http': { url: endpoint } } })}\ncursor-agent mcp login pockymoe-http`],
  ] : [];
  return { local, http };
}
