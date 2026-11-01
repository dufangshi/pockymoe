import type { ThreadDto, WorkspaceDto } from '@remote-codex/shared';
import type { ActivityRequest } from '../host';

export interface DesktopDevice {
  id: string;
  name: string;
  connected: boolean;
  lastHeartbeatAt: string | null;
}
export interface DesktopWorkspace extends Pick<WorkspaceDto, 'id' | 'label' | 'isFavorite' | 'createdAt' | 'lastOpenedAt'> {
  key: string;
  deviceId: string;
}
export interface DesktopThread extends Pick<ThreadDto,
  'id' | 'workspaceId' | 'title' | 'provider' | 'status' | 'isPinned' | 'updatedAt' |
  'createdAt' | 'lastTurnStartedAt' | 'lastTurnCompletedAt' | 'activeTurnId' |
  'parentThreadId' | 'rootThreadId' | 'agentName' | 'closedAt'> {
  key: string;
  deviceId: string;
  lastViewedAt: string | null;
  requests: ActivityRequest[];
  resultSummary: string | null;
}
export interface DesktopIndex {
  accountId: string;
  devices: readonly DesktopDevice[];
  workspaces: readonly DesktopWorkspace[];
  threads: readonly DesktopThread[];
  hydrated: boolean;
  refreshing: boolean;
  error: string | null;
}
export const entityKey = (deviceId: string, id: string) => `${deviceId}:${id}`;
export const threadPath = (thread: Pick<DesktopThread, 'deviceId' | 'id'>) =>
  `/devices/${encodeURIComponent(thread.deviceId)}/threads/${encodeURIComponent(thread.id)}`;
export const unread = (thread: DesktopThread) =>
  Boolean(thread.lastTurnCompletedAt && thread.lastTurnCompletedAt > (thread.lastViewedAt ?? ''));
export const running = (thread: DesktopThread) => thread.status === 'running' || thread.status === 'recovering';
export const failed = (thread: DesktopThread) => thread.status === 'failed' || thread.status === 'system_error';
export const needsAttention = (thread: DesktopThread, now = Date.now()) =>
  thread.requests.length > 0 || (failed(thread) && unread(thread) && now - Date.parse(thread.lastTurnCompletedAt ?? thread.updatedAt) < 86_400_000);
export const active = (thread: DesktopThread, now = Date.now()) => needsAttention(thread, now) || running(thread) || unread(thread);
export function plainSummary(value: string | null | undefined) {
  return (value ?? '').split('\n')[0]!.replace(/<[^>]*>/g, '').replace(/[`*_#]/g, '').replace(/\s+/g, ' ').trim().slice(0, 140);
}
