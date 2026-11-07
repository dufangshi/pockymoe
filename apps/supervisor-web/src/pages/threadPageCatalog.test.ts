import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError, fetchAgentBackendAgents, fetchAgentBackendModelsFor, fetchAgentBackendStatus, fetchThreadModels } from '../lib/api';
import { fetchEnvironments, fetchThreadEnvironment } from '../lib/environmentApi';
import { environmentsFixture, isolatedEnvironment } from '../test/environmentFixtures';
import { requestThreadPageCatalog } from './threadPageCatalog';

vi.mock('../lib/api', async original => ({
  ...await original<typeof import('../lib/api')>(),
  fetchAgentBackendAgents: vi.fn(), fetchAgentBackendModelsFor: vi.fn(), fetchAgentBackendStatus: vi.fn(), fetchThreadModels: vi.fn(),
}));
vi.mock('../lib/environmentApi', async original => ({
  ...await original<typeof import('../lib/environmentApi')>(),
  fetchEnvironments: vi.fn(), fetchThreadEnvironment: vi.fn(),
}));
const apiError = (status: number) => new ApiError(status, { code: 'not_found', message: 'Fixture request failed' });
const model = { id: 'thread-model', model: 'thread-model', displayName: 'Thread model', description: '', isDefault: true, hidden: false, supportedReasoningEfforts: [], defaultReasoningEffort: null };
const hostStatus: Awaited<ReturnType<typeof fetchAgentBackendStatus>> = {
  provider: 'acp', displayName: 'Host ACP', description: 'Host-only metadata', enabled: true, isDefault: true,
  status: { state: 'ready', transport: 'stdio', lastStartedAt: null, lastError: null, restartCount: 0 },
  capabilities: {
    sessions: { list: true, read: true, resume: true, importLocal: true },
    turns: { start: true, streamInput: true, steer: true, interrupt: true, compact: true },
    branching: { fork: true, hardRollback: true, resumeAt: true, rewindFiles: true },
    controls: { planMode: true, permissionRequests: true, sandboxMode: true, performanceMode: true, goals: true },
    management: { models: true, mcpStatus: true, skills: true, hooks: true, hookTrust: true, hostConfigFiles: true, providerSettings: true },
    usage: { contextWindow: true, tokenUsage: true, costUsd: true },
  },
  managementSchema: { hostConfigFiles: [{ name: 'host-config', label: 'Host-only config', description: 'Host-only configuration' }], toolboxItems: [], hookCommandTemplates: [], providerConfigFormat: 'json', mcpConfigFormat: 'none', configArchives: true, buildRestart: true },
  installation: { packageName: null, installed: true, installedVersion: 'fixture', latestVersion: null, installCommand: null, updateCommand: null, busy: false, lastError: null },
};
const binding = (environment: typeof isolatedEnvironment | null) => ({ environment, supportedHarnesses: ['codex'], limitations: [] });
const input: Parameters<typeof requestThreadPageCatalog>[0] = { provider: 'acp', agentId: 'codex', cwd: '/project', threadId: 'thread-one', deviceId: 'device-one', isCurrent: () => true };
const settle = (overrides: Partial<typeof input> = {}) => {
  const { modelRequest, agentRequest, statusRequest } = requestThreadPageCatalog({ ...input, ...overrides });
  return Promise.all([modelRequest, agentRequest, statusRequest]);
};
function expectNoHostDiscovery() {
  expect(fetchAgentBackendAgents).not.toHaveBeenCalled();
  expect(fetchAgentBackendModelsFor).not.toHaveBeenCalled();
  expect(fetchAgentBackendStatus).not.toHaveBeenCalled();
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(fetchThreadEnvironment).mockResolvedValue(binding(isolatedEnvironment));
  vi.mocked(fetchEnvironments).mockResolvedValue(environmentsFixture());
  vi.mocked(fetchThreadModels).mockResolvedValue([model]);
  vi.mocked(fetchAgentBackendAgents).mockResolvedValue([model]);
  vi.mocked(fetchAgentBackendModelsFor).mockResolvedValue([model]);
  vi.mocked(fetchAgentBackendStatus).mockResolvedValue(hostStatus);
});

it('uses scoped models for an isolated ACP thread and never discovers Host agents', async () => {
  expect(await settle()).toEqual([[model], [], null]);
  expect(fetchThreadEnvironment).toHaveBeenCalledWith('thread-one', 'device-one');
  expect(fetchThreadModels).toHaveBeenCalledWith('thread-one');
  expectNoHostDiscovery();
});
it('never treats an isolated thread-model 404 as permission to probe the Host', async () => {
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  expect(await settle()).toEqual([[], [], null]);
  expectNoHostDiscovery();
});
it.each([401, 403, 404, 500])('fails closed when binding lookup returns %i while the environment API exists', async status => {
  vi.mocked(fetchThreadEnvironment).mockRejectedValue(apiError(status));
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  expect(await settle()).toEqual([[], [], null]);
  expectNoHostDiscovery();
});
it('fails closed on a binding network failure or malformed binding', async () => {
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  vi.mocked(fetchThreadEnvironment).mockRejectedValueOnce(new TypeError('Network unavailable'))
    .mockResolvedValueOnce({} as Awaited<ReturnType<typeof fetchThreadEnvironment>>);
  await settle(); await settle();
  expectNoHostDiscovery();
});
it.each([401, 403, 500])('does not infer legacy Host when the environment capability check returns %i', async status => {
  vi.mocked(fetchThreadEnvironment).mockRejectedValue(apiError(404));
  vi.mocked(fetchEnvironments).mockRejectedValue(apiError(status));
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  expect(await settle()).toEqual([[], [], null]);
  expectNoHostDiscovery();
});
it('keeps the legacy Host path only when both environment endpoints return 404', async () => {
  vi.mocked(fetchThreadEnvironment).mockRejectedValue(apiError(404));
  vi.mocked(fetchEnvironments).mockRejectedValue(apiError(404));
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  expect(await settle()).toEqual([[model], [model], hostStatus]);
  expect(fetchEnvironments).toHaveBeenCalledWith('device-one');
  expect(fetchAgentBackendModelsFor).toHaveBeenCalledWith('acp', { agentId: 'codex', cwd: '/project' });
  expect(fetchAgentBackendAgents).toHaveBeenCalledWith('acp');
});
it('keeps Host agents and the old thread-model 404 fallback after an explicit null binding', async () => {
  vi.mocked(fetchThreadEnvironment).mockResolvedValue(binding(null));
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  expect(await settle()).toEqual([[model], [model], hostStatus]);
  expect(fetchEnvironments).not.toHaveBeenCalled();
  expect(fetchAgentBackendAgents).toHaveBeenCalledWith('acp');
  expect(fetchAgentBackendModelsFor).toHaveBeenCalledTimes(1);
});
it('does not fall back on a thread-model server failure even for a confirmed Host thread', async () => {
  vi.mocked(fetchThreadEnvironment).mockResolvedValue(binding(null));
  const failure = apiError(500);
  vi.mocked(fetchThreadModels).mockRejectedValue(failure);
  const { modelRequest, agentRequest, statusRequest } = requestThreadPageCatalog(input);
  await expect(modelRequest).rejects.toBe(failure); await agentRequest; await statusRequest;
  expect(fetchAgentBackendModelsFor).not.toHaveBeenCalled();
});
it('waits for the binding and suppresses late Host requests after navigation changes', async () => {
  let resolve!: (value: ReturnType<typeof binding>) => void;
  vi.mocked(fetchThreadEnvironment).mockImplementation(() => new Promise(done => { resolve = done; }));
  vi.mocked(fetchThreadModels).mockRejectedValue(apiError(404));
  let current = true;
  const pending = settle({ isCurrent: () => current });
  expectNoHostDiscovery();
  current = false;
  resolve(binding(null));
  expect(await pending).toEqual([[], [], null]);
  expectNoHostDiscovery();
});
it('does not probe without a trusted thread context', async () => {
  expect(await settle({ threadId: undefined })).toEqual([[], [], null]);
  expect(fetchThreadEnvironment).not.toHaveBeenCalled();
  expect(fetchThreadModels).not.toHaveBeenCalled();
  expectNoHostDiscovery();
});
