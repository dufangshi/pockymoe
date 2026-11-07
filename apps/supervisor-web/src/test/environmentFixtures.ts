import type { EnvironmentConnectionDto, EnvironmentDto, EnvironmentsDto } from '../lib/environmentApi';

export const environmentConnection: EnvironmentConnectionDto = {
  id: 'dokploy-local', name: 'Local Dokploy', baseUrl: 'https://dokploy.example.test', configured: true,
  dokployEnvironmentId: 'development', image: 'example/agent:1.0', storageNode: 'local', dockerHost: null,
  allowedRoots: ['/projects'], cpuLimit: 2, memoryLimitBytes: 2 * 1024 ** 3, pidsLimit: 512, nofileLimit: 1024,
};
export const isolatedEnvironment: EnvironmentDto = {
  id: 'env-one', name: 'Feature worktree', connectionId: 'dokploy-local', provider: 'dokploy', threadId: 'thread-one',
  generation: 1, state: 'stopped', workspacePath: '/projects/pockymoe', worktreePath: '/projects/pockymoe.worktrees/feature',
  mounts: [{ source: '/projects/pockymoe.worktrees/feature', target: '/workspace', readOnly: false }],
  limits: { cpu: 2, memoryBytes: 2 * 1024 ** 3, pids: 512, nofile: 1024 }, observedLimits: null, diskQuotaSupported: false,
  createdAt: '2026-11-07T00:00:00Z', updatedAt: '2026-11-07T00:00:00Z',
};
export const environmentsFixture = (): EnvironmentsDto => ({
  connections: [structuredClone(environmentConnection)], environments: [structuredClone(isolatedEnvironment)],
  budget: { maxRunning: 3, maxCpu: 8, maxMemoryBytes: 8 * 1024 ** 3, reservedCpu: 1, reservedMemoryBytes: 1024 ** 3 },
});
