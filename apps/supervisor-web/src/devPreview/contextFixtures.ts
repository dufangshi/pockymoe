import type { ContextInspectionSnapshotDto, ThreadContextInspectionDto, ThreadContextUsageDto } from '../../../../packages/shared/src/index';

/** Recorded from real sessions: Claude CLI 2.1.316 at 32.9k of 1M (paths anonymized)
    and Codex 0.165.0 at 57.2k of 828.4k. Each scenario's title says what is recorded
    and what is demo data: `high`, `moved-on` and `unsupported` have made-up numbers,
    to reach what those sessions did not. */
const ago = (seconds: number) => new Date(Date.now() - seconds * 1000).toISOString();
const claudeUsed = [
  { id: 'system_prompt', label: 'System prompt', kind: 'used', tokens: 2334 },
  { id: 'system_tools', label: 'System tools', kind: 'used', tokens: 9961 },
  { id: 'mcp_instructions', label: 'MCP server instructions', kind: 'used', tokens: 717 },
  { id: 'memory_files', label: 'Memory files', kind: 'used', tokens: 1529 },
  { id: 'skills', label: 'Skills', kind: 'used', tokens: 2309 },
  { id: 'messages', label: 'Messages', kind: 'used', tokens: 16048 },
] as const;
const claudeGroups: ContextInspectionSnapshotDto['groups'] = [
  { id: 'mcp_tools', label: 'MCP tools', items: [
    { name: "mcp__claude_ai_Claude_Docs__batch", tokens: 162, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__create", tokens: 252, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__delete", tokens: 326, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__export", tokens: 496, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__guide", tokens: 206, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__query", tokens: 179, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__read", tokens: 320, source: "claude_ai_Claude_Docs", loaded: false },
    { name: "mcp__claude_ai_Claude_Docs__update", tokens: 293, source: "claude_ai_Claude_Docs", loaded: false },
  ] },
  { id: 'memory_files', label: 'Memory files', items: [
    { name: "/home/me/my-project/CLAUDE.md", path: "/home/me/my-project/CLAUDE.md", tokens: 1347, source: "Project" },
    { name: "/home/me/my-project/packages/app/CLAUDE.md", path: "/home/me/my-project/packages/app/CLAUDE.md", tokens: 49, source: "Project" },
    { name: "/home/me/.claude/projects/my-project/memory/MEMORY.md", path: "/home/me/.claude/projects/my-project/memory/MEMORY.md", tokens: 133, source: "AutoMem" },
  ] },
  { id: 'mcp_instructions', label: 'MCP instructions', items: [
    { name: "claude.ai Claude Docs", tokens: 717 },
  ] },
  { id: 'skills', label: 'Skills', items: [
    { name: "context-probe", tokens: 28, source: "projectSettings" },
    { name: "dataviz", tokens: 482, source: "built-in" },
    { name: "update-config", tokens: 236, source: "built-in" },
    { name: "keybindings-help", tokens: 83, source: "built-in" },
    { name: "code-review", tokens: 402, source: "built-in" },
    { name: "simplify", tokens: 63, source: "built-in" },
    { name: "fewer-permission-prompts", tokens: 64, source: "built-in" },
    { name: "loop", tokens: 115, source: "built-in" },
    { name: "schedule", tokens: 127, source: "built-in" },
    { name: "claude-api", tokens: 361, source: "built-in" },
    { name: "workflow-authoring", tokens: 84, source: "built-in" },
    { name: "run", tokens: 123, source: "built-in" },
    { name: "plugin-authoring", tokens: 89, source: "built-in" },
    { name: "init", tokens: 22, source: "built-in" },
    { name: "security-review", tokens: 30, source: "built-in" },
  ] },
];
const claude = (source: 'claudeSummary' | 'claudeFull'): ContextInspectionSnapshotDto => ({
  provider: 'claude',
  model: 'claude-opus-5-5',
  source,
  precision: source === 'claudeFull' ? 'reported' : 'estimated',
  usedTokens: 32898,
  windowTokens: 1_000_000,
  updatedAt: ago(14),
  autoCompactThreshold: 967_000,
  autoCompactEnabled: true,
  categories: [
    ...claudeUsed.map(entry => ({ ...entry, precision: source === 'claudeFull' ? 'reported' as const : 'estimated' as const })),
    { id: 'autocompact_buffer', label: 'Autocompact buffer', kind: 'buffer', tokens: 33000 },
    { id: 'free_space', label: 'Free space', kind: 'free', tokens: 934102 },
    { id: 'system_tools_deferred', label: 'System tools (deferred)', kind: 'deferred', tokens: 15579 },
    { id: 'mcp_tools_deferred', label: 'MCP tools (deferred)', kind: 'deferred', tokens: 2234 },
  ],
  groups: claudeGroups,
});
const codex: ContextInspectionSnapshotDto = {
  provider: 'codex',
  model: 'gpt-6.1-sol',
  source: 'codexTrace',
  precision: 'estimated',
  usedTokens: 57159,
  windowTokens: 828_400,
  updatedAt: ago(3),
  categories: [
    { id: 'messages', kind: 'used', tokens: 26050, precision: 'estimated' },
    { id: 'memory_files', kind: 'used', tokens: 874, precision: 'estimated' },
    { id: 'skills', kind: 'used', tokens: 14474, precision: 'estimated' },
    { id: 'system_prompt', kind: 'used', tokens: 5298, precision: 'estimated' },
    { id: 'system_tools', kind: 'used', tokens: 7206, precision: 'estimated' },
    { id: 'unattributed', kind: 'used', tokens: 3257, precision: 'estimated' },
    { id: 'free_space', kind: 'free', tokens: 771241 },
  ],
  groups: [{ id: 'system_tools', items: [
    { name: "functions", tokens: 4813 },
    { name: "collaboration", tokens: 1194 },
    { name: "mcp__cua_repl", tokens: 1087 },
    { name: "clock", tokens: 112 },
  ] }],
};
const high: ContextInspectionSnapshotDto = {
  ...claude('claudeFull'),
  usedTokens: 171_000,
  windowTokens: 200_000,
  autoCompactThreshold: 167_000,
  categories: [
    { id: 'system_prompt', label: 'System prompt', kind: 'used', tokens: 2334, precision: 'reported' },
    { id: 'system_tools', label: 'System tools', kind: 'used', tokens: 9961, precision: 'reported' },
    { id: 'mcp_tools', label: 'MCP tools', kind: 'used', tokens: 12340, precision: 'reported' },
    { id: 'memory_files', label: 'Memory files', kind: 'used', tokens: 6210, precision: 'reported' },
    { id: 'skills', label: 'Skills', kind: 'used', tokens: 4309, precision: 'reported' },
    { id: 'custom_agents', label: 'Custom agents', kind: 'used', tokens: 1846, precision: 'reported' },
    { id: 'messages', label: 'Messages', kind: 'used', tokens: 134000, precision: 'reported' },
    { id: 'autocompact_buffer', label: 'Autocompact buffer', kind: 'buffer', tokens: 29000, precision: 'reported' },
    { id: 'free_space', label: 'Free space', kind: 'free', tokens: 0, precision: 'reported' },
    { id: 'system_tools_deferred', label: 'System tools (deferred)', kind: 'deferred', tokens: 15579, precision: 'reported' },
  ],
};
const usage = (used: number, window: number, seconds = 14, details?: ContextInspectionSnapshotDto): ThreadContextUsageDto => ({
  availability: 'available', tokensInContextWindow: used, modelContextWindow: window,
  remainingPercent: Math.round(100 * Math.max(0, window - used) / window), updatedAt: ago(seconds), ...(details ? { details } : {}),
});
const answer = (snapshot: ContextInspectionSnapshotDto | null, extra: Partial<ThreadContextInspectionDto> = {}): ThreadContextInspectionDto =>
  ({ enabled: true, supported: true, availability: 'available', snapshot, ...extra });

export interface ContextPreviewScenario {
  id: string;
  title: string;
  model: { id: string; name: string; agent: string };
  usage: ThreadContextUsageDto | null;
  /** What GET /context answers, by `detail`; `pending` never answers, `error` is a 500. */
  summary: ThreadContextInspectionDto | 'pending' | 'error';
  full?: ThreadContextInspectionDto | 'error';
  settingsEnabled?: boolean;
}

const claudeModel = { id: 'claude-opus-5-5', name: 'Opus 5.5', agent: 'claude' };
const codexModel = { id: 'gpt-6.1-sol', name: 'GPT-6.1 Sol', agent: 'codex' };
export const contextPreviewScenarios: ContextPreviewScenario[] = [
  { id: 'claude-summary', title: 'Recorded · Claude summary, Full count available', model: claudeModel, usage: usage(32898, 1_000_000, 14, claude('claudeSummary')), summary: answer(claude('claudeSummary')), full: answer({ ...claude('claudeFull'), updatedAt: ago(0) }), settingsEnabled: true },
  { id: 'claude-full', title: 'Recorded · Claude full count', model: claudeModel, usage: usage(32898, 1_000_000, 14, claude('claudeFull')), summary: answer(claude('claudeFull')), full: answer({ ...claude('claudeFull'), updatedAt: ago(0) }), settingsEnabled: true },
  { id: 'codex', title: 'Recorded · Codex local estimate with unattributed', model: codexModel, usage: usage(57159, 828_400, 3, codex), summary: answer(codex), settingsEnabled: true },
  { id: 'high', title: 'Demo data · Claude 86%, past the auto-compact mark', model: claudeModel, usage: usage(171_000, 200_000, 8, high), summary: answer(high), settingsEnabled: true },
  { id: 'waiting', title: 'Demo state, recorded total · On, no request analyzed yet', model: codexModel, usage: usage(45391, 828_400), summary: answer(null), settingsEnabled: true },
  { id: 'stale', title: 'Demo state, recorded data · Harness released, last result marked stale', model: claudeModel, usage: usage(32898, 1_000_000, 3600), summary: answer({ ...claude('claudeSummary'), stale: true, updatedAt: ago(3600) }, { availability: 'unavailable', reason: 'The harness for this thread is not loaded.' }), settingsEnabled: true },
  { id: 'moved-on', title: 'Demo data · Compacted since the analysis, categories older than the total', model: claudeModel, usage: usage(9200, 200_000, 2, { ...high, updatedAt: ago(90) }), summary: answer({ ...high, updatedAt: ago(90) }), settingsEnabled: true },
  { id: 'disabled', title: 'Demo state, recorded total · Setting off', model: codexModel, usage: usage(57159, 828_400), summary: { enabled: false, supported: true, availability: 'disabled', snapshot: null }, settingsEnabled: false },
  { id: 'unsupported', title: 'Demo data · Other harness, total only', model: { id: 'grok-4', name: 'Grok 4', agent: 'grok' }, usage: usage(18400, 256_000), summary: { enabled: true, supported: false, availability: 'unavailable', snapshot: null, reason: 'ACP Grok reports totals only.' }, settingsEnabled: true },
  { id: 'loading', title: 'Demo state, recorded total · Reading', model: claudeModel, usage: usage(32898, 1_000_000), summary: 'pending', settingsEnabled: true },
  { id: 'failed', title: 'Demo state, recorded total · Read failed, with Retry', model: claudeModel, usage: usage(32898, 1_000_000), summary: 'error', settingsEnabled: true },
  { id: 'refresh-failed', title: 'Demo state, recorded data · Full count fails, last result stays', model: claudeModel, usage: usage(32898, 1_000_000, 14, claude('claudeSummary')), summary: answer(claude('claudeSummary')), full: 'error', settingsEnabled: true },
  { id: 'empty', title: 'Demo state · New thread, nothing reported', model: claudeModel, usage: null, summary: answer(null), settingsEnabled: true },
];
