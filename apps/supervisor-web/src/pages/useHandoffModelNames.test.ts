// @vitest-environment jsdom
import { renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ThreadTurnDto } from '@pockymoe/shared';
import { deviceRequest } from '../lib/api';
import { useHandoffModelNames } from './useHandoffModelNames';
vi.mock('../lib/api', () => ({ deviceRequest: vi.fn() }));
const turns = [{provider:'acp',items:[{harnessSwitch:{fromAgentId:'codex',agentId:'claude'}}]}] as ThreadTurnDto[];
it('resolves aliases through the historical harness catalogs and isolates device names', async () => {
  vi.mocked(deviceRequest).mockImplementation(async (device) => {
    if (device === 'offline') throw Error('offline');
    return [{id:'claude-haiku-5-5',model:'haiku',displayName:`${device} Haiku 5.5`}];
  });
  const {result,rerender} = renderHook(({device}) => useHandoffModelNames(turns,device), {initialProps:{device:'device-a'}});
  await waitFor(() => expect(result.current('claude','haiku')).toBe('device-a Haiku 5.5'));
  expect(result.current('claude','claude-haiku-5-5')).toBe('device-a Haiku 5.5');
  expect(result.current('claude','unknown')).toBeUndefined();
  expect(deviceRequest).toHaveBeenCalledWith('device-a','api/agent-runtimes/acp/models?agentId=codex');
  rerender({device:'offline'});
  expect(result.current('claude','haiku')).toBeUndefined();
  await waitFor(() => expect(deviceRequest).toHaveBeenCalledWith('offline','api/agent-runtimes/acp/models?agentId=claude'));
});

it('resolves standalone switches before another prompt exists', async () => {
  vi.mocked(deviceRequest).mockResolvedValue([{id:'haiku',model:'haiku',displayName:'Haiku 5.5'}]);
  const entries = [{id:'switch',kind:'harnessSwitch' as const,text:'',harnessSwitch:{fromAgentId:'codex',agentId:'claude',turns:0}}];
  const {result} = renderHook(() => useHandoffModelNames([], 'device-b', entries));
  await waitFor(() => expect(result.current('claude','haiku')).toBe('Haiku 5.5'));
});

it('reads an ACP side of a switch from the ACP adapter catalog, by agent key', async () => {
  vi.mocked(deviceRequest).mockReset();
  vi.mocked(deviceRequest).mockImplementation(async (_device, path) =>
    String(path).includes('acp%3Acodex') ? [{id:'gpt-6.1-sol',model:'gpt-6.1-sol',displayName:'ACP 6.1 Sol'}] : [{id:'gpt-6.1-sol',model:'gpt-6.1-sol',displayName:'6.1 Sol'}]);
  const entries = [{id:'switch',kind:'harnessSwitch' as const,text:'',harnessSwitch:{fromAgentId:'codex',fromHarnessProtocol:'native' as const,agentId:'codex',harnessProtocol:'acp' as const,turns:2}}];
  const {result} = renderHook(() => useHandoffModelNames([], 'device-c', entries));
  await waitFor(() => expect(result.current('acp:codex','gpt-6.1-sol')).toBe('ACP 6.1 Sol'));
  expect(result.current('codex','gpt-6.1-sol')).toBe('6.1 Sol');
  expect(deviceRequest).toHaveBeenCalledWith('device-c','api/agent-runtimes/acp/models?agentId=acp%3Acodex');
});
