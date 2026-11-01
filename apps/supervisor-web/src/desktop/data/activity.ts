import type { ActivityItem, ActivitySnapshot } from '../host';
import { failed, plainSummary, recentFailure, running, unread, type DesktopIndex } from './types';

const priority = { needsInput: 0, failed: 1, interrupted: 2, completed: 3, running: 4, idle: 5 } as const;
/** Native alerts, the map attention strip and sidebar all derive from this order. */
export function buildActivityItems(index: DesktopIndex, now = Date.now()): ActivityItem[] {
  const devices = new Map(index.devices.map(device => [device.id, device]));
  const workspaces = new Map(index.workspaces.map(workspace => [workspace.key, workspace]));
  const threads = new Map(index.threads.map(thread => [thread.key, thread]));
  return index.threads.flatMap<ActivityItem>(thread => {
    if (devices.get(thread.deviceId)?.activityStale) return [];
    const state = thread.requests.length ? 'needsInput' : recentFailure(thread, now) ? 'failed' : thread.status === 'interrupted' && unread(thread) && now - Date.parse(thread.lastTurnCompletedAt ?? thread.updatedAt) < 86_400_000 ? 'interrupted' : running(thread) ? 'running' : unread(thread) && !failed(thread) ? 'completed' : 'idle';
    if (state === 'idle') return [];
    const request = thread.requests[0];
    const rootKey = thread.rootThreadId ? `${thread.deviceId}:${thread.rootThreadId}` : thread.key;
    const rootTitle = threads.get(rootKey)?.title ?? thread.title;
    return [{ key: thread.key, deviceId: thread.deviceId, deviceName: devices.get(thread.deviceId)?.name ?? '',
      workspaceId: thread.workspaceId, workspaceLabel: workspaces.get(`${thread.deviceId}:${thread.workspaceId}`)?.label ?? '',
      threadId: thread.id, title: plainSummary(thread.title),
      rootThreadKey: rootKey, rootThreadTitle: plainSummary(rootTitle), agentName: thread.agentName ?? null, state,
      since: (state === 'needsInput' ? request?.createdAt : state === 'running' ? thread.lastTurnStartedAt : thread.lastTurnCompletedAt) ?? thread.updatedAt,
      turnId: thread.activeTurnId ?? thread.lastTurnId, unread: state === 'failed' || state === 'interrupted' ? true : unread(thread),
      request: request ? { id: request.id, kind: request.kind, summary: plainSummary(request.summary) } : null,
      resultSummary: plainSummary(thread.resultSummary) || null,
      durationMs: ['completed', 'failed', 'interrupted'].includes(state) && thread.lastTurnStartedAt && thread.lastTurnCompletedAt ? Math.max(0, Date.parse(thread.lastTurnCompletedAt) - Date.parse(thread.lastTurnStartedAt)) : null,
    }];
  }).sort((a, b) => priority[a.state] - priority[b.state] || a.since.localeCompare(b.since) || a.key.localeCompare(b.key));
}
export function buildActivitySnapshot(index: DesktopIndex, focusedThreadKey: string | null, hasFocus: boolean, now = Date.now()): ActivitySnapshot {
  const synced = index.syncedDeviceIds && new Set(index.syncedDeviceIds);
  const threads = new Map(index.threads.map(thread => [thread.key, thread]));
  const recentKeys = index.recentThreadKeys ?? [...index.threads].filter(thread => thread.lastViewedAt).sort((a, b) => b.lastViewedAt!.localeCompare(a.lastViewedAt!)).map(thread => thread.key);
  const seen = new Set<string>();
  const recent = recentKeys.flatMap(key => {
    const opened = threads.get(key); if (!opened) return [];
    const thread = threads.get(`${opened.deviceId}:${opened.rootThreadId}`) ?? opened;
    if (seen.has(thread.key)) return []; seen.add(thread.key);
    return [{ key: thread.key, deviceId: thread.deviceId, deviceName: index.devices.find(device => device.id === thread.deviceId)?.name ?? '',
      workspaceLabel: index.workspaces.find(workspace => workspace.key === `${thread.deviceId}:${thread.workspaceId}`)?.label ?? '', threadId: thread.id, title: plainSummary(thread.title) }];
  }).slice(0, 5);
  return { version: 1, generatedAt: new Date(now).toISOString(), accountId: index.accountId, focusedThreadKey: hasFocus ? focusedThreadKey : null,
    items: buildActivityItems(index, now).filter(item => !synced || synced.has(item.deviceId)), recent,
    syncedDeviceIds: [...(index.syncedDeviceIds ?? [])] };
}
export function attentionFromActivity(items: readonly ActivityItem[]) {
  const roots = new Set<string>();
  return items.filter(item => item.state !== 'running' && item.state !== 'interrupted').filter(item => { const key = item.rootThreadKey ?? item.key; if (roots.has(key)) return false; roots.add(key); return true; }).map(item => ({
    key: item.key, kind: item.request?.kind ?? (item.state === 'failed' ? 'failed' as const : 'unread' as const),
    title: item.title, device: item.deviceName, workspace: item.workspaceLabel, since: item.since,
    ...(item.request ? { requestId: item.request.id, summary: item.request.summary } : item.resultSummary ? { summary: item.resultSummary } : {}),
  }));
}
