import { expect, it } from 'vitest';
import type { ModelOptionDto, ThreadDto } from '@pockymoe/shared';
import { composerAgentLabel } from './threadAgentLabel';

const option = (model: string, displayName: string) => ({ id: model, model, displayName }) as ModelOptionDto;
const thread = (fields: Partial<ThreadDto>) => ({ provider: 'acp', agentId: 'codex', ...fields }) as ThreadDto;

it('names native agents as the device does and ACP variants "ACP · <agent>"', () => {
  const options = [option('codex', 'Codex'), option('claude', 'Claude Code'), option('acp:claude', 'Claude Code')];
  expect(composerAgentLabel(thread({ harnessProtocol: 'native' }), options)).toBe('Codex');
  expect(composerAgentLabel(thread({ agentId: 'claude', harnessProtocol: 'acp' }), options)).toBe('ACP · Claude Code');
  // Without an entry for the ACP variant, the agent's own entry names it.
  expect(composerAgentLabel(thread({ harnessProtocol: 'acp' }), [option('codex', 'OpenAI Codex')])).toBe('ACP · OpenAI Codex');
  expect(composerAgentLabel(thread({}), [option('codex', 'OpenAI Codex')])).toBe('OpenAI Codex');
  expect(composerAgentLabel(thread({ agentId: 'opencode' }))).toBe('OpenCode');
  expect(composerAgentLabel(thread({ agentId: null }))).toBeNull();
});
