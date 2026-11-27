import type { AgentBackendIdDto } from '@pockymoe/shared';

const AGENT_SCHEMES: Record<string, string> = {
  codex: 'codex',
  claude: 'claude',
  'claude-code': 'claude',
  claudecode: 'claude',
  grok: 'grok',
  'grok-build': 'grok',
  xai: 'grok',
  cursor: 'cursor',
  'cursor-agent': 'cursor',
  gemini: 'gemini',
  antigravity: 'antigravity',
  agy: 'antigravity',
  copilot: 'copilot',
  'github-copilot': 'copilot',
  opencode: 'opencode',
  'open-code': 'opencode',
  deepseek: 'deepseek',
  dsh: 'deepseek',
  acp: 'acp',
};

const BOUND_PROVIDERS = new Set(['codex', 'claude', 'opencode']);

export interface ParsedSessionRef {
  rawId: string;
  agentId: string | null;
}

export function parseSessionRef(input: string): ParsedSessionRef {
  const trimmed = input
    .trim()
    .replace(/^["'`<[({]+/, '')
    .replace(/["'`)\]}>]+$/, '')
    .replace(/[/\\]+$/, '')
    .trim();
  if (!trimmed) {
    return { rawId: '', agentId: null };
  }

  const scoped = trimmed.match(/^([a-z0-9-]+)::(.+)$/i);
  const scopedAgent = scoped?.[1]?.toLowerCase();
  const scopedId = scoped?.[2];
  if (scopedAgent && scopedId && AGENT_SCHEMES[scopedAgent]) {
    return {
      rawId: scopedId,
      agentId: AGENT_SCHEMES[scopedAgent] ?? null,
    };
  }

  const uri = trimmed.match(/^([a-z][a-z0-9+.-]*):\/\/(.+)$/i);
  const scheme = uri?.[1]?.toLowerCase();
  const remainder = uri?.[2];
  if (scheme && remainder) {
    const rest = remainder.split(/[?#]/, 1)[0]?.replace(/^\/\//, '') ?? remainder;
    const parts = rest.split(/[/\\]/).filter(Boolean);
    const rawId = parts.at(-1) ?? rest;
    const fromScheme = AGENT_SCHEMES[scheme] ?? null;
    const fromPath =
      parts.map((part) => AGENT_SCHEMES[part.toLowerCase()]).find(Boolean) ?? null;
    return { rawId, agentId: fromScheme ?? fromPath };
  }

  const parts = trimmed.split(/[/\\?#]/).filter(Boolean);
  return {
    rawId: parts.at(-1) ?? trimmed,
    agentId: AGENT_SCHEMES[trimmed.toLowerCase()] ?? null,
  };
}

export function providerForImportedAgent(
  agentId: string | null,
  backends: AgentBackendIdDto[],
): AgentBackendIdDto | null {
  if (!agentId) {
    return null;
  }
  if (BOUND_PROVIDERS.has(agentId) && backends.includes(agentId as AgentBackendIdDto)) {
    return agentId as AgentBackendIdDto;
  }
  if (backends.includes('acp')) {
    return 'acp';
  }
  return backends[0] ?? null;
}
