import type { DeviceMetricsDto, ThreadActionRequestDto, ThreadActivityResponseDto, ThreadDto, WorkspaceDto } from '@pockymoe/shared';
export interface DesktopDevice { id: string; name: string; connected: boolean; lastHeartbeatAt: string | null; cachedAt?: string }
export interface DesktopWorkspace extends WorkspaceDto { key: string; deviceId: string }
export interface DesktopRequest { id: string; kind: 'approval' | 'input'; summary: string; createdAt: string }
export interface DesktopThread extends ThreadDto {
  key: string; deviceId: string; lastViewedAt: string | null; lastTurnId: string | null;
  requests: DesktopRequest[]; resultSummary: string | null;
  /** Existing history is not a new inbox: only completions observed after indexing are unread until viewed. */
  firstIndexedCompletedAt?: string | null;
}
export interface DesktopExtra {
  latencyMs?: number; runtimeVersion?: string; latestVersion?: string; updateAvailable?: boolean;
  metrics?: DeviceMetricsDto; todayCostUsd?: number; syncing?: boolean;
}
export interface DesktopIndex {
  accountId: string;
  devices: readonly DesktopDevice[]; workspaces: readonly DesktopWorkspace[]; threads: readonly DesktopThread[];
  activity: Readonly<Record<string, ThreadActivityResponseDto>>;
  extras: Readonly<Record<string, DesktopExtra>>;
  hydrated: boolean; refreshing: boolean; error: string | null;
  pulseKeys: readonly string[];
}
export const entityKey = (deviceId: string, id: string) => `${deviceId}:${id}`;
export const unread = (thread: DesktopThread) => Boolean(thread.lastTurnCompletedAt && thread.lastTurnCompletedAt > (thread.lastViewedAt ?? thread.firstIndexedCompletedAt ?? ''));
export const running = (thread: DesktopThread) => thread.status === 'running' || thread.status === 'recovering';
export const failed = (thread: Pick<ThreadDto, 'status'>) => thread.status === 'failed' || thread.status === 'system_error';
export const recentFailure = (thread: DesktopThread, now = Date.now()) => failed(thread) && (thread.lastTurnCompletedAt ?? thread.updatedAt) > (thread.lastViewedAt ?? '') && now - Date.parse(thread.lastTurnCompletedAt ?? thread.updatedAt) < 86_400_000;
export const needsAttention = (thread: DesktopThread, now = Date.now()) => thread.requests.length > 0 || recentFailure(thread, now);
export function plainSummary(value: string | null | undefined, limit = 140) {
  return (value ?? '').split('\n')[0]!.replace(/<[^>]*>/g, '').replace(/[`*#]/g, '').replace(/\bBearer\s+[A-Za-z0-9._~+/=-]+/gi, 'Bearer [redacted]')
    .replace(/\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]+)/g, '[redacted]')
    .replace(/(\b(?:api[_-]?key|access[_-]?token|password|secret)\b\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;]+)/gi, '$1[redacted]')
    .replace(/\s+/g, ' ').trim().slice(0, limit);
}
export const summarizeRequest = (value: ThreadActionRequestDto): DesktopRequest => ({ id: value.id, kind: value.kind === 'requestUserInput' ? 'input' : 'approval', createdAt: value.createdAt, summary: plainSummary(value.description || value.title) });
export function summarizeThread(deviceId: string, thread: ThreadDto, previous?: DesktopThread): DesktopThread {
  return { ...thread, key: entityKey(deviceId, thread.id), deviceId,
    lastViewedAt: previous?.lastViewedAt ?? null, lastTurnId: thread.activeTurnId ?? previous?.lastTurnId ?? null,
    ...(!previous ? { firstIndexedCompletedAt: failed(thread) || thread.status === 'interrupted' ? null : thread.lastTurnCompletedAt } : previous.firstIndexedCompletedAt !== undefined ? { firstIndexedCompletedAt: previous.firstIndexedCompletedAt } : {}),
    requests: previous?.requests ?? [], resultSummary: plainSummary(thread.lastError || thread.summaryText) || null };
}
