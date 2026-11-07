import { deviceRequest } from './api';

// Kept in the Web until the Rust protocol DTOs are exported to @pockymoe/shared.
// The public connection deliberately has no credential field.
export interface EnvironmentConnectionDto {
  id: string;
  name: string;
  baseUrl: string;
  configured: boolean;
  dokployEnvironmentId: string;
  image: string;
  storageNode: 'local';
  dockerHost?: string | null;
  allowedRoots: string[];
  allowedEgressHosts?: string[];
  cpuLimit: number;
  memoryLimitBytes: number;
  pidsLimit: number;
  nofileLimit: number;
}
export type UpdateEnvironmentConnection = Omit<EnvironmentConnectionDto, 'id' | 'configured'> & { apiKey?: string };
export interface EnvironmentBudgetDto {
  maxRunning: number;
  maxCpu: number;
  maxMemoryBytes: number;
  reservedCpu: number;
  reservedMemoryBytes: number;
}
export interface EnvironmentLimits { cpu: number; memoryBytes: number; pids: number; nofile: number }
export type EnvironmentState = 'creating' | 'stopped' | 'starting' | 'running' | 'stopping' | 'error' | 'destroyed' | 'interrupted';
export interface EnvironmentDto {
  id: string;
  name: string;
  connectionId: string;
  provider: 'dokploy';
  threadId: string;
  generation: number;
  state: EnvironmentState;
  containerId?: string | null;
  composeId?: string | null;
  workspacePath: string;
  worktreePath?: string | null;
  mounts: { source: string; target: string; readOnly: boolean }[];
  limits: EnvironmentLimits;
  observedLimits?: EnvironmentLimits | null;
  lastError?: string | null;
  diskQuotaSupported: false;
  createdAt: string;
  updatedAt: string;
}
export interface EnvironmentsDto {
  connections: EnvironmentConnectionDto[];
  environments: EnvironmentDto[];
  budget: EnvironmentBudgetDto;
}
export interface ThreadEnvironmentDto {
  environment: EnvironmentDto | null;
  supportedHarnesses: string[];
  limitations: string[];
}
export type EnvironmentAction = 'start' | 'stop' | 'rebuild' | 'destroy';
export interface EnvironmentRequestDto {
  id: string; threadId: string; digest: string;
  state: 'pending' | 'waitingIdle' | 'provisioning' | 'ready' | 'failed' | 'rejected' | 'cancelled';
  environmentId: string | null; lastError: string | null; approvedAt: string | null;
  continuationQueued: boolean; createdAt: string; updatedAt: string;
  spec: {
    connection: EnvironmentConnectionDto; threadId: string; workspaceId: string;
    workspacePath: string; workingPath: string; harness: string; model: string | null; reasoningEffort: string | null;
    name: string; limits: EnvironmentLimits; egressHosts: string[];
    authentication: { codex: 'apiBroker' | 'nativeFile'; claude: 'apiBroker' | 'nativeFile' };
    credentialMounts: { source: string; target: string; readOnly: boolean; purpose: 'generic' | 'codex' | 'claude' | 'github'; environment: Record<string, string> }[];
    credentialMountKinds: ('directory' | 'file')[];
    handoff: string; migration: 'sameThreadFreshSession';
  };
}
export function fetchEnvironmentRequests(deviceId?: string | null, threadId?: string, signal?: AbortSignal) {
  return deviceRequest<EnvironmentRequestDto[]>(deviceId, `api/environment-requests${threadId ? `?threadId=${encodeURIComponent(threadId)}` : ''}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, threadId);
}
export function decideEnvironmentRequest(request: Pick<EnvironmentRequestDto, 'id' | 'digest' | 'threadId'>, action: 'approve' | 'reject' | 'retry' | 'recoverHost', deviceId?: string | null) {
  return deviceRequest<EnvironmentRequestDto>(deviceId, `api/environment-requests/${encodeURIComponent(request.id)}/${action}`, { method: 'POST', body: JSON.stringify({ digest: request.digest }) }, request.threadId);
}
const resourceRevisions = new Map<string, number>();
const resourceKey = (id: string, deviceId?: string | null) => JSON.stringify([deviceId ?? 'local', id]);
export const environmentResourceRevision = (id: string, deviceId?: string | null) => resourceRevisions.get(resourceKey(id, deviceId)) ?? 0;
export function notifyEnvironmentChanged(threadId: string, deviceId?: string | null) {
  const revision = environmentResourceRevision(threadId, deviceId) + 1;
  resourceRevisions.set(resourceKey(threadId, deviceId), revision);
  window.dispatchEvent(new CustomEvent('pockymoe:environment-changed', { detail: { threadId, deviceId, revision } }));
}
const root = 'api/environments';
const threadPath = (id: string) => `api/threads/${encodeURIComponent(id)}/environment`;
const json = (method: string, body?: unknown): RequestInit => ({ method, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });

export function fetchEnvironments(deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<EnvironmentsDto>(deviceId, root, { cache: 'no-store', ...(signal ? { signal } : {}) });
}
export function saveEnvironmentConnection(id: string, input: UpdateEnvironmentConnection, deviceId?: string | null) {
  // An empty write-only field means keep the stored key, never erase it.
  const { apiKey, ...fields } = input;
  return deviceRequest<EnvironmentConnectionDto>(deviceId, `${root}/connections/${encodeURIComponent(id)}`, json('PUT', { ...fields, ...(apiKey ? { apiKey } : {}) }));
}
export function testEnvironmentConnection(id: string, deviceId?: string | null) {
  return deviceRequest<{ ok: boolean; detail: string }>(deviceId, `${root}/connections/${encodeURIComponent(id)}/test`, json('POST'));
}
export function saveEnvironmentBudget(input: EnvironmentBudgetDto, deviceId?: string | null) {
  return deviceRequest<EnvironmentBudgetDto>(deviceId, `${root}/budget`, json('PUT', input));
}
export function createEnvironment(input: { connectionId: string; threadId: string; name?: string }, deviceId?: string | null) {
  return deviceRequest<EnvironmentDto>(deviceId, root, json('POST', input), input.threadId);
}
export function actOnEnvironment(id: string, action: EnvironmentAction, deviceId?: string | null) {
  return deviceRequest<EnvironmentDto>(deviceId, `${root}/${encodeURIComponent(id)}/${action}`, json('POST'));
}
export function fetchThreadEnvironment(id: string, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<ThreadEnvironmentDto>(deviceId, threadPath(id), { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function bindThreadEnvironment(id: string, environmentId: string | null, deviceId?: string | null, resetSession?: true) {
  return deviceRequest<ThreadEnvironmentDto>(deviceId, threadPath(id), json('PUT', { environmentId, ...(resetSession ? { resetSession } : {}) }), id);
}
