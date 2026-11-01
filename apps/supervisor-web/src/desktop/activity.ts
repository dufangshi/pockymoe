import type { ActivityItem, ActivitySnapshot, HostCommand, HostCommandResult, RemoteCodexHost } from './host';
import { failed, needsAttention, plainSummary, running, threadPath, unread, type DesktopIndex } from './data/types';
import type { DesktopIndexStore } from './data/store';
import type { ThreadDetailDto, ThreadDto } from '@remote-codex/shared';
import { deviceRequest } from '../lib/api';

export function buildActivitySnapshot(index: DesktopIndex, focusedThreadKey: string | null, hasFocus: boolean, now = Date.now()): ActivitySnapshot {
  return {
    version: 1, generatedAt: new Date(now).toISOString(), accountId: index.accountId,
    focusedThreadKey: hasFocus ? focusedThreadKey : null,
    items: index.threads.flatMap<ActivityItem>(thread => {
      const state = thread.requests.length ? 'needsInput' : failed(thread) && needsAttention(thread, now) ? 'failed' : running(thread) ? 'running' : unread(thread) && !failed(thread) ? 'completed' : 'idle';
      if (state === 'idle') return [];
      const workspace = index.workspaces.find(row => row.id === thread.workspaceId && row.deviceId === thread.deviceId);
      const request = thread.requests[0];
      return [{
        key: thread.key, deviceId: thread.deviceId, deviceName: index.devices.find(row => row.id === thread.deviceId)?.name ?? '',
        workspaceId: thread.workspaceId, workspaceLabel: workspace?.label ?? '', threadId: thread.id,
        title: plainSummary(thread.title), agentName: thread.agentName ?? null, state,
        since: (state === 'needsInput' ? request?.createdAt ?? thread.updatedAt : state === 'running' ? thread.lastTurnStartedAt : thread.lastTurnCompletedAt) ?? thread.updatedAt,
        turnId: thread.activeTurnId ?? thread.lastTurnId ?? null, unread: unread(thread),
        request: request ? { id: request.id, kind: request.kind, summary: plainSummary(request.summary) } : null,
        resultSummary: plainSummary(thread.resultSummary) || null,
        durationMs: thread.lastTurnStartedAt && thread.lastTurnCompletedAt ? Math.max(0, Date.parse(thread.lastTurnCompletedAt) - Date.parse(thread.lastTurnStartedAt)) : null,
      }];
    }).sort((a, b) => a.key.localeCompare(b.key)),
  };
}

/** Sends only meaningful changes, at most once per 250 ms, including focus changes. */
export function syncActivity(store: DesktopIndexStore, host: RemoteCodexHost, focused: () => string | null, badgeEnabled = true) {
  let previous = ''; let timer: ReturnType<typeof setTimeout> | undefined; let lastSent = 0;
  const flush = () => {
    timer = undefined;
    const snapshot = buildActivitySnapshot(store.getSnapshot(), focused(), document.hasFocus());
    const signature = JSON.stringify({ ...snapshot, generatedAt: '' });
    if (signature === previous) return;
    previous = signature; lastSent = Date.now(); host.activity.sync(snapshot);
    const attention = snapshot.items.filter(item => item.state === 'needsInput' || item.state === 'failed').length;
    const runningCount = snapshot.items.filter(item => item.state === 'running').length;
    if (badgeEnabled) host.badge.set(attention || runningCount, attention ? 'attention' : runningCount ? 'running' : 'none');
  };
  const schedule = () => { timer ??= setTimeout(flush, Math.max(0, 250 - (Date.now() - lastSent))); };
  const unsubscribe = store.subscribe(schedule);
  window.addEventListener('focus', schedule); window.addEventListener('blur', schedule);
  window.addEventListener('remote-codex:desktop-route', schedule);
  // Age out failed items even when the account has no other activity.
  const aging = setInterval(schedule, 30_000); schedule();
  return () => { unsubscribe(); clearTimeout(timer); clearInterval(aging); window.removeEventListener('focus', schedule); window.removeEventListener('blur', schedule); window.removeEventListener('remote-codex:desktop-route', schedule); };
}
export interface CommandDependencies {
  store: DesktopIndexStore;
  navigate(path: string): void;
  openPalette(): void;
  newThread(): void;
  request: typeof deviceRequest;
}
export async function handleHostCommand(command: HostCommand, deps: CommandDependencies): Promise<HostCommandResult> {
  try {
    switch (command.type) {
      case 'navigate': {
        if (!command.path.startsWith('/') || command.path.startsWith('//') || command.path.includes('\\')) throw new Error('Invalid navigation path.');
        deps.navigate(command.path); break;
      }
      case 'markRead': deps.store.markRead(command.key); break;
      case 'resolveRequest': {
        const detail = await deps.store.loadDetail(command.deviceId, command.threadId, true);
        const action = detail.pendingRequests.find(request => request.id === command.requestId);
        if (!action) throw new Error('This request has already been resolved.');
        if (action.kind === 'requestUserInput') throw new Error('Open the thread to answer this input request.');
        const question = action.questions[0];
        if (!question || action.questions.length !== 1) throw new Error('Open the thread to review this request.');
        const pattern = command.decision === 'approve' ? /^(allow|approve|accept|yes|允许|批准|同意)(?:\b|\s|$)/i : /^(deny|reject|decline|no|拒绝|不允许)(?:\b|\s|$)/i;
        const options = question.options ?? [];
        const once = command.decision === 'approve' ? options.find(option => /allow.once|approve.once/i.test(option.description ?? '')) : undefined;
        const option = once ?? options.find(option => pattern.test(option.description ?? '')) ?? options.find(option => pattern.test(option.label));
        if (!option) throw new Error('This approval requires review in the thread.');
        await deps.request<ThreadDetailDto>(command.deviceId, `/api/threads/${encodeURIComponent(command.threadId)}/requests/${encodeURIComponent(command.requestId)}/respond`, { method: 'POST', body: JSON.stringify({ answers: { [question.id]: { answers: [option.label] } } }) });
        await deps.store.loadDetail(command.deviceId, command.threadId, true); break;
      }
      case 'reply': {
        if (!command.text.trim()) throw new Error('Reply must not be empty.');
        const thread = await deps.request<ThreadDto>(command.deviceId, `/api/threads/${encodeURIComponent(command.threadId)}/prompt`, { method: 'POST', body: JSON.stringify({ prompt: command.text, clientRequestId: command.id }) });
        deps.store.patchThread(command.deviceId, thread); break;
      }
      case 'openPalette': deps.openPalette(); break;
      case 'newThread': deps.newThread(); break;
      case 'openDevices': deps.navigate('/desktop/devices'); break;
    }
    return { ok: true };
  } catch (error) { return { ok: false, error: error instanceof Error ? error.message : 'The action could not be completed.' }; }
}
export function focusedKeyFromPath(path: string): string | null {
  const match = path.match(/^\/devices\/([^/]+)\/threads\/([^/]+)$/);
  return match && !['new', 'import'].includes(match[2]!) ? `${decodeURIComponent(match[1]!)}:${decodeURIComponent(match[2]!)}` : null;
}
export const openThreadCommand = (index: DesktopIndex, key: string) => {
  const thread = index.threads.find(row => row.key === key); return thread ? threadPath(thread) : null;
};
