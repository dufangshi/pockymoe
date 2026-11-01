import type { RelayPortalSummaryDto, ThreadActionRequestDto, ThreadDetailDto, ThreadDto, ThreadEventEnvelope, WorkspaceDto } from '@remote-codex/shared';
import { deviceRequest, fetchRelayPortal, request } from '../../lib/api';
import { encryptedRelaySocket } from '../../lib/relayTransport';
import { clearIndex, readIndex, writeIndex } from './cache';
import { entityKey, plainSummary, type DesktopDevice, type DesktopIndex, type DesktopThread } from './types';

/** Reuses every unchanged row, and the array itself when membership/order are unchanged. */
export function mergeRows<T extends object>(previous: readonly T[], incoming: readonly T[], key: (row: T) => string): readonly T[] {
  const old = new Map(previous.map(row => [key(row), row]));
  const next = incoming.map(row => {
    const existing = old.get(key(row));
    return existing && JSON.stringify(existing) === JSON.stringify(row) ? existing : row;
  });
  return previous.length === next.length && next.every((row, i) => previous[i] === row) ? previous : next;
}
const acceptedEvents = new Set(['thread.updated', 'thread.turn.started', 'thread.turn.completed', 'thread.turn.failed', 'thread.request.created', 'thread.request.resolved']);
export const isIndexEvent = (type: string) => acceptedEvents.has(type);
const summarizeRequest = (value: ThreadActionRequestDto) => ({
  id: value.id, kind: value.kind === 'requestUserInput' ? 'input' as const : 'approval' as const,
  summary: plainSummary(value.description || value.title),
});
export function summarizeThread(deviceId: string, row: ThreadDto, previous?: DesktopThread): DesktopThread {
  return {
    key: entityKey(deviceId, row.id), deviceId, id: row.id, workspaceId: row.workspaceId,
    title: row.title, provider: row.provider, status: row.status, isPinned: row.isPinned,
    updatedAt: row.updatedAt, createdAt: row.createdAt, lastTurnStartedAt: row.lastTurnStartedAt,
    lastTurnCompletedAt: row.lastTurnCompletedAt, activeTurnId: row.activeTurnId,
    parentThreadId: row.parentThreadId ?? null, rootThreadId: row.rootThreadId ?? null, agentName: row.agentName ?? null,
    closedAt: row.closedAt ?? null, lastViewedAt: previous?.lastViewedAt ?? null,
    requests: previous?.requests ?? [], resultSummary: plainSummary(row.lastError || row.summaryText) || null,
  };
}
export interface DesktopDataAdapter {
  portal(): Promise<RelayPortalSummaryDto>;
  get<T>(deviceId: string, path: string, init?: RequestInit): Promise<T>;
  presence(deviceId: string, signal: AbortSignal): Promise<{ connected: boolean }>;
  socket(deviceId: string): WebSocket;
}
const adapter: DesktopDataAdapter = {
  portal: fetchRelayPortal,
  get: deviceRequest,
  presence: (id, signal) => request(`/relay/devices/${encodeURIComponent(id)}/presence`, { signal, cache: 'no-store' }),
  socket: (id) => encryptedRelaySocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/relay/devices/${encodeURIComponent(id)}/ws`),
};

export class DesktopIndexStore {
  private state: DesktopIndex;
  private listeners = new Set<() => void>();
  private sockets = new Map<string, WebSocket>();
  private retries = new Map<string, ReturnType<typeof setTimeout>>();
  private refreshes = new Map<string, Promise<void>>();
  private details = new Map<string, { at: number; detail: ThreadDetailDto }>();
  private detailRequests = new Map<string, Promise<ThreadDetailDto>>();
  private persistTimer?: ReturnType<typeof setTimeout>;
  private presenceTimer?: ReturnType<typeof setInterval>;
  private controllers = new Set<AbortController>();
  private stopped = false;
  private generation = 0;
  constructor(readonly accountId: string, private io: DesktopDataAdapter = adapter) {
    this.state = { accountId, devices: [], workspaces: [], threads: [], hydrated: false, refreshing: false, error: null };
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(patch: Partial<DesktopIndex>) {
    if (this.stopped) return;
    if (Object.entries(patch).every(([key, value]) => this.state[key as keyof DesktopIndex] === value)) return;
    this.state = { ...this.state, ...patch };
    this.listeners.forEach(listener => listener());
    clearTimeout(this.persistTimer);
    this.persistTimer = setTimeout(() => { void writeIndex(this.state); }, 100);
  }
  async hydrate() {
    const cached = await readIndex(this.accountId);
    if (this.stopped) return;
    if (cached?.accountId === this.accountId) this.state = { ...cached, refreshing: false, hydrated: true };
    else this.state = { ...this.state, hydrated: true };
  }
  start() {
    this.stopped = false;
    void this.refresh();
    this.presenceTimer = setInterval(() => { void this.probePresence(); }, 30_000);
    window.addEventListener('remote-codex:logout', this.logout);
  }
  stop() {
    this.stopped = true;
    this.generation++;
    clearInterval(this.presenceTimer);
    clearTimeout(this.persistTimer);
    this.retries.forEach(clearTimeout); this.retries.clear();
    this.sockets.forEach(socket => socket.close()); this.sockets.clear();
    this.controllers.forEach(controller => controller.abort()); this.controllers.clear();
    window.removeEventListener('remote-codex:logout', this.logout);
  }
  logout = () => {
    this.publish({ devices: [], workspaces: [], threads: [] });
    this.stop(); this.details.clear(); this.detailRequests.clear();
    void clearIndex(this.accountId);
  };
  async refresh() {
    const generation = this.generation;
    this.publish({ refreshing: true, error: null });
    try {
      const portal = await this.io.portal();
      if (this.stopped || generation !== this.generation) return;
      const devices = portal.devices.filter(device => device.ownerUserId === this.accountId).map(({ id, name, connected, lastHeartbeatAt }) => ({ id, name, connected, lastHeartbeatAt }));
      const ids = new Set(devices.map(device => device.id));
      this.publish({ devices: mergeRows(this.state.devices, devices, row => row.id),
        workspaces: this.state.workspaces.filter(row => ids.has(row.deviceId)),
        threads: this.state.threads.filter(row => ids.has(row.deviceId)),
      });
      for (const [id, socket] of this.sockets) if (!ids.has(id) || !devices.find(device => device.id === id)?.connected) { this.sockets.delete(id); socket.close(); }
      await Promise.allSettled(devices.filter(device => device.connected).map(async device => {
        this.connect(device.id);
        await this.refreshDevice(device.id);
      }));
    } catch (error) {
      this.publish({ error: error instanceof Error ? error.message : '设备索引暂时不可用' });
    } finally { if (generation === this.generation) this.publish({ refreshing: false }); }
  }
  refreshDevice = (deviceId: string): Promise<void> => {
    const pending = this.refreshes.get(deviceId);
    if (pending) return pending;
    const generation = this.generation;
    const controller = new AbortController(); this.controllers.add(controller);
    const task = (async () => {
      try {
        const [workspaces, threads] = await Promise.all([
          this.io.get<WorkspaceDto[]>(deviceId, '/api/workspaces', { signal: controller.signal }),
          this.io.get<ThreadDto[]>(deviceId, '/api/threads?includeAgentThreads=true', { signal: controller.signal }),
        ]);
        if (this.stopped || generation !== this.generation) return;
        const old = new Map(this.state.threads.map(row => [row.key, row]));
        const nextThreads = threads.map(row => summarizeThread(deviceId, row, old.get(entityKey(deviceId, row.id))));
        this.publish({
          workspaces: mergeRows(this.state.workspaces, [
            ...this.state.workspaces.filter(row => row.deviceId !== deviceId),
            ...workspaces.map(({ id, label, isFavorite, createdAt, lastOpenedAt }) => ({ key: entityKey(deviceId, id), deviceId, id, label, isFavorite, createdAt, lastOpenedAt })),
          ], row => row.key),
          threads: mergeRows(this.state.threads, [...this.state.threads.filter(row => row.deviceId !== deviceId), ...nextThreads], row => row.key),
        });
        // Requests are only available on thread details. Bound fan-out and fetch
        // all open threads, including idle ones with an outstanding input request.
        let cursor = 0;
        const targets = nextThreads.filter(row => !row.closedAt);
        await Promise.all(Array.from({ length: Math.min(6, targets.length) }, async () => {
          while (cursor < targets.length && !this.stopped && generation === this.generation) {
            const row = targets[cursor++]!;
            try { await this.loadDetail(deviceId, row.id, true); } catch { /* Keep the last known request state while unreachable. */ }
          }
        }));
      } catch (error) {
        if (!controller.signal.aborted) this.publish({ error: error instanceof Error ? error.message : '设备不可用' });
      } finally { this.controllers.delete(controller); }
    })();
    this.refreshes.set(deviceId, task);
    void task.finally(() => this.refreshes.delete(deviceId));
    return task;
  };
  private updateThread(key: string, update: (thread: DesktopThread) => DesktopThread) {
    const next = this.state.threads.map(row => row.key === key ? update(row) : row);
    this.publish({ threads: mergeRows(this.state.threads, next, row => row.key) });
  }
  markRead = (key: string) => this.updateThread(key, row => ({ ...row, lastViewedAt: new Date().toISOString() }));
  patchThread = (deviceId: string, thread: ThreadDto) => this.updateThread(entityKey(deviceId, thread.id), row => summarizeThread(deviceId, thread, row));
  async loadDetail(deviceId: string, threadId: string, force = false): Promise<ThreadDetailDto> {
    const key = entityKey(deviceId, threadId);
    const pending = this.detailRequests.get(key); if (pending) return pending;
    const cached = this.details.get(key);
    if (!force && cached && Date.now() - cached.at < 30_000) {
      this.details.delete(key); this.details.set(key, cached); return cached.detail;
    }
    const generation = this.generation;
    const requestState = this.state.threads.find(row => row.key === key)?.requests;
    const controller = new AbortController(); this.controllers.add(controller);
    const task = this.io.get<ThreadDetailDto>(deviceId, `/api/threads/${encodeURIComponent(threadId)}?view=summary&limit=3`, { signal: controller.signal }).then(detail => {
      if (this.stopped || generation !== this.generation) return detail;
      this.details.delete(key); this.details.set(key, { at: Date.now(), detail });
      while (this.details.size > 8) this.details.delete(this.details.keys().next().value!);
      this.updateThread(key, row => {
        // A response initiated before a live event cannot overwrite newer status.
        const next = detail.thread.updatedAt >= row.updatedAt ? summarizeThread(deviceId, detail.thread, row) : row;
        return { ...next, requests: row.requests === requestState ? detail.pendingRequests.map(summarizeRequest) : row.requests };
      });
      return detail;
    }).finally(() => { this.controllers.delete(controller); this.detailRequests.delete(key); });
    this.detailRequests.set(key, task); return task;
  }
  prefetchDetail = (deviceId: string, threadId: string) => { void this.loadDetail(deviceId, threadId).catch(() => undefined); };
  applyEvent(deviceId: string, event: ThreadEventEnvelope) {
    if (!isIndexEvent(event.type)) return;
    const key = entityKey(deviceId, event.threadId);
    if (!this.state.threads.some(row => row.key === key)) { void this.refreshDevice(deviceId); return; }
    this.updateThread(key, row => {
      if (event.timestamp < row.updatedAt) return row;
      switch (event.type) {
        case 'thread.updated': {
          const status = event.payload.status;
          const allowed = ['recovering', 'idle', 'running', 'interrupted', 'failed', 'not_loaded', 'system_error'];
          return { ...row, title: event.payload.title ?? row.title, status: status && allowed.includes(status) ? status as DesktopThread['status'] : row.status, updatedAt: event.timestamp };
        }
        case 'thread.turn.started': return { ...row, status: 'running', activeTurnId: event.payload.turnId, lastTurnStartedAt: event.timestamp, updatedAt: event.timestamp };
        case 'thread.turn.completed': return { ...row, status: event.payload.status === 'failed' ? 'failed' : 'idle', activeTurnId: null, lastTurnCompletedAt: event.timestamp, updatedAt: event.timestamp, resultSummary: plainSummary(event.payload.error) || row.resultSummary };
        case 'thread.turn.failed': return { ...row, status: event.payload.willRetry ? 'recovering' : 'failed', activeTurnId: event.payload.willRetry ? row.activeTurnId : null, lastTurnCompletedAt: event.payload.willRetry ? row.lastTurnCompletedAt : event.timestamp, updatedAt: event.timestamp, resultSummary: plainSummary(event.payload.error) };
        case 'thread.request.created': return { ...row, updatedAt: event.timestamp, requests: [...row.requests.filter(request => request.id !== event.payload.request.id), summarizeRequest(event.payload.request)] };
        case 'thread.request.resolved': return { ...row, updatedAt: event.timestamp, requests: row.requests.filter(request => request.id !== event.payload.requestId) };
        default: return row;
      }
    });
  }
  private connect(deviceId: string, attempt = 0) {
    if (this.stopped || this.sockets.has(deviceId) || !this.state.devices.find(device => device.id === deviceId)?.connected) return;
    const socket = this.io.socket(deviceId); this.sockets.set(deviceId, socket);
    socket.addEventListener('message', message => {
      try {
        const event = JSON.parse(String(message.data)) as ThreadEventEnvelope;
        // High-volume item/delta events never enter the store or notify React.
        if (isIndexEvent(event.type)) this.applyEvent(deviceId, event);
      } catch { /* Ignore malformed envelopes. */ }
    });
    socket.addEventListener('open', () => { attempt = 0; void this.refreshDevice(deviceId); });
    socket.addEventListener('close', () => {
      if (this.sockets.get(deviceId) !== socket) return;
      this.sockets.delete(deviceId);
      if (this.stopped) return;
      const delay = [1000, 2000, 5000, 15000][Math.min(attempt, 3)]!;
      this.retries.set(deviceId, setTimeout(() => { this.retries.delete(deviceId); this.connect(deviceId, attempt + 1); }, delay));
    });
  }
  private async probePresence() {
    if (this.stopped || document.visibilityState === 'hidden') return;
    await Promise.allSettled(this.state.devices.map(async device => {
      const controller = new AbortController(); this.controllers.add(controller);
      const timeout = setTimeout(() => controller.abort(), 5000);
      try {
        const result = await this.io.presence(device.id, controller.signal);
        if (controller.signal.aborted || this.stopped) return;
        this.setPresence(device, result.connected);
      } catch { if (!this.stopped) this.setPresence(device, false); }
      finally { clearTimeout(timeout); this.controllers.delete(controller); }
    }));
  }
  private setPresence(device: DesktopDevice, connected: boolean) {
    this.publish({ devices: mergeRows(this.state.devices, this.state.devices.map(row => row.id === device.id ? { ...row, connected } : row), row => row.id) });
    if (connected) { this.connect(device.id); if (!device.connected) void this.refreshDevice(device.id); }
    else { const socket = this.sockets.get(device.id); this.sockets.delete(device.id); socket?.close(); }
  }
}
