import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ModelOptionDto, ThreadDto } from '@pockymoe/shared';
import { useModelPickerSource } from './useModelPickerSource';

const api = vi.hoisted(() => ({ deviceRequest: vi.fn(), interruptThread: vi.fn() }));
const environments = vi.hoisted(() => ({ fetchThreadEnvironment: vi.fn() }));
vi.mock('../lib/api', () => api);
vi.mock('../lib/environmentApi', () => environments);

const thread = { id: 'thread', provider: 'acp', agentId: 'codex', status: 'idle' } as ThreadDto;
const agent = (id: string, displayName: string, availability: string) =>
  ({ id, model: id, displayName, acpAgent: { availability } }) as unknown as ModelOptionDto;
const agents = [agent('codex', 'Codex', 'ready'), agent('claude', 'Claude Agent', 'ready'), agent('gemini', 'Gemini', 'base_missing')];
const models = [
  { id: 'haiku', model: 'haiku', displayName: 'Haiku 5.5', isDefault: false, hidden: false },
  { id: 'opus', model: 'opus', displayName: 'Opus 5.5', isDefault: true, hidden: false },
] as ModelOptionDto[];

beforeEach(() => {
  vi.clearAllMocks();
  api.interruptThread.mockResolvedValue({});
  environments.fetchThreadEnvironment.mockResolvedValue({ environment: null });
  api.deviceRequest.mockImplementation(async (_device: string, path: string, init?: RequestInit) => {
    if (path === 'api/config/model-pricing') return { models: { haiku: { contextWindowTokens: 200000, inputUsdPerMillion: 1, outputUsdPerMillion: 5 } } };
    if (path === 'api/agent-runtimes/acp/agents') return agents;
    if (path.startsWith('api/agent-runtimes/acp/models?')) return models;
    if (path.endsWith('/preview')) return { text: 'Saved history', turns: 2, estimatedTokens: 50, resumesSession: false };
    if (path.endsWith('/switch')) return { ...thread, agentId: JSON.parse(String(init?.body)).agentId };
    if (path.endsWith('/fork')) return { ...thread, id: 'fork' };
    return {};
  });
});

describe('useModelPickerSource', () => {
  it('offers nothing to a reader who cannot switch', async () => {
    const { result } = renderHook(() => useModelPickerSource({
      thread, deviceId: 'device-2', cwd: '/w', agentOptions: agents, canSwitch: false, onSwitched: vi.fn(),
    }));
    expect(result.current).toBeUndefined();
    expect(api.deviceRequest).not.toHaveBeenCalled();
  });

  it('marks an ACP Codex thread by its agent key and offers native Codex beside it', async () => {
    const protocolAgents = [
      { ...agent('codex', 'Codex', 'ready'), acpAgent: { availability: 'ready', harnessProtocol: 'native' } },
      { ...agent('acp:codex', 'Codex', 'ready'), acpAgent: { availability: 'ready', harnessProtocol: 'acp', baseAgentId: 'codex' } },
    ] as unknown as ModelOptionDto[];
    const onSwitched = vi.fn();
    const { result } = renderHook(() => useModelPickerSource({
      thread: { ...thread, harnessProtocol: 'acp' }, deviceId: null, cwd: '/w', agentOptions: protocolAgents, canSwitch: true, onSwitched,
    }));
    await waitFor(() => expect(result.current?.switching).toBeDefined());
    const switching = result.current!.switching!;
    expect(switching.currentAgentId).toBe('acp:codex');
    expect(switching.agents).toEqual([{ id: 'codex', name: 'Codex' }, { id: 'acp:codex', name: 'ACP · Codex' }]);
    await switching.switchTo({ agentId: 'codex', model: 'gpt-6.1-sol', reasoningEffort: null, stop: false });
    const body = JSON.parse(String(api.deviceRequest.mock.calls.find((call) => String(call[1]).endsWith('/switch'))![2].body));
    expect(body).toEqual({ provider: 'acp', agentId: 'codex', model: 'gpt-6.1-sol', reasoningEffort: null });
  });

  it('lists ready agents and reads models, history and facts on the thread device', async () => {
    const { result } = renderHook(() => useModelPickerSource({
      thread, deviceId: 'device-2', cwd: '/w', agentOptions: agents, canSwitch: true, onSwitched: vi.fn(), onForked: vi.fn(),
    }));
    await waitFor(() => expect(result.current?.facts).toBeDefined());
    const switching = result.current!.switching!;
    expect(switching.currentAgentId).toBe('codex');
    expect(switching.agents).toEqual([{ id: 'codex', name: 'Codex' }, { id: 'claude', name: 'Claude Agent' }]);
    expect(result.current!.facts!(models[0]!)).toEqual({ contextWindow: 200000, inputUsdPerMillion: 1, outputUsdPerMillion: 5 });

    await switching.loadModels('claude');
    await switching.loadModels('claude');
    const modelCalls = api.deviceRequest.mock.calls.filter((call) => String(call[1]).includes('/models?'));
    expect(modelCalls).toEqual([['device-2', 'api/agent-runtimes/acp/models?agentId=claude&cwd=%2Fw', { cache: 'no-store' }]]);

    await expect(switching.preview({ agentId: 'claude', model: 'opus' })).resolves.toMatchObject({ turns: 2 });
    const preview = api.deviceRequest.mock.calls.find((call) => String(call[1]).endsWith('/preview'))!;
    expect(preview[0]).toBe('device-2');
    expect(JSON.parse(preview[2].body)).toEqual({ provider: 'acp', agentId: 'claude', model: 'opus', reasoningEffort: null });
  });

  it('switches an idle thread at once and reports the moved thread', async () => {
    const onSwitched = vi.fn();
    const { result } = renderHook(() => useModelPickerSource({
      thread, deviceId: null, cwd: null, agentOptions: agents, canSwitch: true, onSwitched,
    }));
    await result.current!.switching!.switchTo({ agentId: 'claude', model: 'haiku', reasoningEffort: null, stop: false });
    expect(api.interruptThread).not.toHaveBeenCalled();
    const call = api.deviceRequest.mock.calls.find((entry) => String(entry[1]).endsWith('/switch'))!;
    expect(call[1]).toBe('api/threads/thread/harness/switch');
    expect(JSON.parse(call[2].body)).toEqual({ provider: 'acp', agentId: 'claude', model: 'haiku', reasoningEffort: null });
    expect(onSwitched).toHaveBeenCalledWith(expect.objectContaining({ agentId: 'claude' }));
    // The split pane does not fork.
    expect(result.current!.switching!.fork).toBeUndefined();
  });

  it('stops the running turn and lets it settle before switching', async () => {
    const { result } = renderHook(() => useModelPickerSource({
      thread: { ...thread, status: 'running' }, deviceId: 'device-2', cwd: null, agentOptions: agents, canSwitch: true, onSwitched: vi.fn(),
    }));
    await result.current!.switching!.switchTo({ agentId: 'claude', model: 'opus', reasoningEffort: 'high', stop: true });
    expect(api.interruptThread).toHaveBeenCalledWith('thread', {}, 'device-2');
    const paths = api.deviceRequest.mock.calls.map((call) => String(call[1]));
    expect(paths.indexOf('api/threads/thread/release')).toBeGreaterThanOrEqual(0);
    expect(paths.indexOf('api/threads/thread/release')).toBeLessThan(paths.indexOf('api/threads/thread/harness/switch'));
  });

  it('forks into another agent on its default model', async () => {
    const onForked = vi.fn();
    const { result } = renderHook(() => useModelPickerSource({
      thread, deviceId: null, cwd: null, agentOptions: agents, canSwitch: true, onSwitched: vi.fn(), onForked,
    }));
    await result.current!.switching!.fork!({ agentId: 'claude', turnId: 'turn-1' });
    const call = api.deviceRequest.mock.calls.find((entry) => String(entry[1]).endsWith('/fork'))!;
    expect(JSON.parse(call[2].body)).toEqual({ turnId: 'turn-1', target: { provider: 'acp', agentId: 'claude', model: 'opus', reasoningEffort: null } });
    expect(onForked).toHaveBeenCalledWith(expect.objectContaining({ id: 'fork' }));
  });

  it('reads agents itself for a host thread, and none for an isolated one', async () => {
    const { result, rerender } = renderHook(({ id }) => useModelPickerSource({
      thread: { ...thread, id }, deviceId: 'device-3', cwd: null, canSwitch: true, onSwitched: vi.fn(),
    }), { initialProps: { id: 'host' } });
    await waitFor(() => expect(result.current?.switching?.agents.map((entry) => entry.id)).toEqual(['codex', 'claude']));
    expect(environments.fetchThreadEnvironment).toHaveBeenCalledWith('host', 'device-3');

    environments.fetchThreadEnvironment.mockResolvedValue({ environment: { id: 'env' } });
    rerender({ id: 'isolated' });
    await waitFor(() => expect(environments.fetchThreadEnvironment).toHaveBeenCalledWith('isolated', 'device-3'));
    await waitFor(() => expect(result.current?.switching).toBeUndefined());
  });
});
