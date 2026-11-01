import type { DeviceMetricsDto, RelayPortalSummaryDto, ThreadActivityResponseDto, ThreadDetailDto, ThreadDto, ThreadEventEnvelope, WorkspaceDto } from '@pockymoe/shared';
import { connectSupervisorEvents, deviceRequest, fetchRelayPortal, request } from '../../lib/api';
import { SupervisorHeartbeat } from '../../lib/supervisorHeartbeat';
import { clearIndex, readIndex, writeIndex } from './cache';
import { mergeRows } from './merge';
import { entityKey, plainSummary, summarizeRequest, summarizeThread, type DesktopExtra, type DesktopIndex, type DesktopThread } from './types';

const accepted = new Set(['thread.updated', 'thread.turn.started', 'thread.turn.completed', 'thread.turn.failed', 'thread.request.created', 'thread.request.resolved']);
export const isIndexEvent = (type: string) => accepted.has(type);
const visible = () => document.visibilityState !== 'hidden';
export interface DesktopDataAdapter {
  portal(): Promise<RelayPortalSummaryDto>;
  get<T>(deviceId: string, path: string, init?: RequestInit): Promise<T>;
  presence(deviceId: string, signal: AbortSignal): Promise<{ connected: boolean }>;
  socket(deviceId: string, receive: (event: ThreadEventEnvelope) => void): WebSocket;
}
const adapter: DesktopDataAdapter = {
  portal: fetchRelayPortal, get: deviceRequest,
  presence: (id, signal) => request(`relay/devices/${encodeURIComponent(id)}/presence`, { signal, cache: 'no-store' }),
  socket: (deviceId, receive) => connectSupervisorEvents(receive, { deviceId, threadId: null }),
};

/** Account-wide summaries, not another thread-page polling loop. Budgets follow
 * docs/web-live-updates.md: lists 60s, event reads >=2s apart; activity is map-only. */
export class DesktopIndexStore {
  private state: DesktopIndex;
  private listeners = new Set<() => void>();
  private sockets = new Map<string, { socket: WebSocket; heartbeat: SupervisorHeartbeat }>();
  private retries = new Map<string, ReturnType<typeof setTimeout>>();
  private pendingLists = new Map<string, ReturnType<typeof setTimeout>>();
  private detailTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private dirtyDetails = new Set<string>();
  private activityTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private requests = new Map<string, Promise<void>>();
  private lastList = new Map<string, number>();
  private lastActivity = new Map<string, number>();
  private lastRuntime = new Map<string, number>();
  private lastExtra = new Map<string, number>();
  private requestHydrated = new Set<string>();
  private dirtyActivity = new Set<string>();
  private details = new Map<string, { detail: ThreadDetailDto; at: number }>();
  private detailRequests = new Map<string, Promise<ThreadDetailDto>>();
  private controllers = new Set<AbortController>();
  private timers: ReturnType<typeof setInterval>[] = [];
  private persistence = Promise.resolve();
  private persistTimer?: ReturnType<typeof setTimeout>;
  private pulseTimer?: ReturnType<typeof setTimeout>;
  private generation = 0;
  private stopped = false;
  private mapVisible = false;
  constructor(readonly accountId: string, private io: DesktopDataAdapter = adapter) {
    this.state = { accountId, devices: [], workspaces: [], threads: [], activity: {}, extras: {}, hydrated: false, refreshing: false, error: null, pulseKeys: [] };
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(patch: Partial<DesktopIndex>) {
    if (this.stopped || Object.entries(patch).every(([key, value]) => this.state[key as keyof DesktopIndex] === value)) return;
    this.state = { ...this.state, ...patch }; this.listeners.forEach(listener => listener());
    clearTimeout(this.persistTimer);
    this.persistTimer = setTimeout(() => { const snapshot = this.state; this.persistence = this.persistence.then(() => writeIndex(snapshot)); }, 150);
  }
  private online(id: string) { return this.state.devices.some(device => device.id === id && device.connected); }
  async hydrate() {
    const cached = await readIndex(this.accountId);
    if (this.stopped) return;
    if (cached?.accountId === this.accountId) this.state = { ...this.state, ...cached, refreshing: false, error: null, pulseKeys: [], hydrated: true };
    else this.state = { ...this.state, hydrated: true };
  }
  start() {
    this.stopped = false; void this.refresh(false);
    this.timers.push(setInterval(() => { if (visible()) void this.refreshPortal().catch(() => undefined); }, 30_000));
    this.timers.push(setInterval(() => {
      if (!visible()) return;
      this.state.devices.filter(device => device.connected).forEach(device => { void this.refreshDevice(device.id); });
    }, 60_000));
    this.timers.push(setInterval(() => {
      if (!visible() || !this.mapVisible) return;
      for (const device of this.state.devices.filter(device => device.connected)) {
        if (Date.now() - (this.lastActivity.get(device.id) ?? 0) >= 60_000) void this.refreshActivity(device.id);
        if (Date.now() - (this.lastExtra.get(device.id) ?? 0) >= 15_000) void this.refreshExtras(device.id);
      }
    }, 5000));
    this.timers.push(setInterval(() => {
      if (!visible()) return;
      for (const { socket, heartbeat } of this.sockets.values()) if (socket.readyState === WebSocket.OPEN) {
        if (heartbeat.timedOut(Date.now())) socket.close();
        else { try { socket.send(JSON.stringify({ type: 'supervisor.ping', nonce: String(Date.now()) })); } catch { socket.close(); } }
      }
    }, 2000));
    document.addEventListener('visibilitychange', this.visibilityChanged);
    window.addEventListener('pockymoe-account-logout', this.logout);
  }
  stop() {
    this.stopped = true; this.generation++;
    this.timers.forEach(clearInterval); this.timers = [];
    this.retries.forEach(clearTimeout); this.retries.clear();
    this.pendingLists.forEach(clearTimeout); this.pendingLists.clear();
    this.activityTimers.forEach(clearTimeout); this.activityTimers.clear();
    this.detailTimers.forEach(clearTimeout); this.detailTimers.clear();
    clearTimeout(this.persistTimer); clearTimeout(this.pulseTimer);
    this.sockets.forEach(({ socket }) => socket.close()); this.sockets.clear();
    this.controllers.forEach(controller => controller.abort()); this.controllers.clear();
    document.removeEventListener('visibilitychange', this.visibilityChanged);
    window.removeEventListener('pockymoe-account-logout', this.logout);
  }
  logout = () => {
    this.publish({ devices: [], workspaces: [], threads: [], activity: {}, extras: {}, pulseKeys: [] });
    this.stop(); this.details.clear(); this.detailRequests.clear();
    void this.persistence.then(() => clearIndex(this.accountId));
  };
  private visibilityChanged = () => {
    if (!visible()) return;
    this.sockets.forEach(({ heartbeat }) => heartbeat.probe(Date.now()));
    void this.refresh(false);
    if (this.mapVisible) this.refreshVisibleActivity();
    for (const key of this.dirtyDetails) { const row = this.state.threads.find(thread => thread.key === key); if (row) this.scheduleDetail(row.deviceId, row.id); }
  };
  setMapVisible = (value: boolean) => {
    if (this.mapVisible === value) return;
    this.mapVisible = value;
    if (value && visible()) this.refreshVisibleActivity();
    if (!value) { this.publish({ pulseKeys: [] }); this.activityTimers.forEach(clearTimeout); this.activityTimers.clear(); }
  };
  private refreshVisibleActivity() {
    for (const device of this.state.devices.filter(device => device.connected)) {
      if (this.dirtyActivity.has(device.id) || Date.now() - (this.lastActivity.get(device.id) ?? 0) >= 60_000) void this.refreshActivity(device.id);
      if (Date.now() - (this.lastExtra.get(device.id) ?? 0) >= 15_000) void this.refreshExtras(device.id);
    }
  }
  async refresh(interactive = true) {
    this.publish({ refreshing: true });
    try {
      await this.refreshPortal();
      await Promise.allSettled(this.state.devices.filter(device => device.connected).map(device => this.refreshDevice(device.id)));
      if (interactive) this.publish({ error: null });
    } catch (error) { if (interactive || !this.state.devices.length) this.publish({ error: error instanceof Error ? error.message : String(error) }); }
    finally { this.publish({ refreshing: false }); }
  }
  private async refreshPortal() {
    if (this.stopped || !visible()) return;
    const generation = this.generation;
    const portal = await this.io.portal();
    if (this.stopped || generation !== this.generation) return;
    const old = new Map(this.state.devices.map(device => [device.id, device]));
    const devices = portal.devices.filter(device => device.ownerUserId === this.accountId).map(({ id, name, connected, lastHeartbeatAt }) => ({ id, name, connected, lastHeartbeatAt, ...(old.get(id)?.cachedAt ? { cachedAt: old.get(id)!.cachedAt! } : {}) }));
    const ids = new Set(devices.map(device => device.id));
    this.publish({ devices: mergeRows(this.state.devices, devices, row => row.id),
      workspaces: this.state.workspaces.filter(row => ids.has(row.deviceId)), threads: this.state.threads.filter(row => ids.has(row.deviceId)) });
    for (const [id, { socket }] of this.sockets) if (!this.online(id)) { this.sockets.delete(id); socket.close(); }
    for (const device of devices) {
      if (device.connected) {
        this.connect(device.id);
        if (!old.has(device.id) || !old.get(device.id)?.connected) void this.refreshDevice(device.id);
      }
      // Same presence protocol as useDevicePresence; a portal lease alone does
      // not prove a device responds. Offline entries keep their cached summaries.
      const controller = new AbortController(); this.controllers.add(controller);
      const timeout = setTimeout(() => controller.abort(), 5000);
      void this.io.presence(device.id, controller.signal).then(result => {
        if (this.stopped || controller.signal.aborted || generation !== this.generation) return;
        const wasOnline = this.online(device.id);
        this.publish({ devices: mergeRows(this.state.devices, this.state.devices.map(row => row.id === device.id ? { ...row, connected: result.connected } : row), row => row.id) });
        if (result.connected && !wasOnline) { this.connect(device.id); void this.refreshDevice(device.id); }
        if (!result.connected) { const entry = this.sockets.get(device.id); this.sockets.delete(device.id); entry?.socket.close(); }
      }).catch(() => undefined).finally(() => { clearTimeout(timeout); this.controllers.delete(controller); });
    }
    if (this.mapVisible) this.refreshVisibleActivity();
  }
  private withSignal<T>(read: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const controller = new AbortController(); this.controllers.add(controller);
    const timeout = setTimeout(() => controller.abort(), 15_000);
    return read(controller.signal).finally(() => { clearTimeout(timeout); this.controllers.delete(controller); });
  }
  refreshDevice = (deviceId: string): Promise<void> => this.single(`list:${deviceId}`, async () => {
    if (this.stopped || !visible() || !this.online(deviceId)) return;
    const generation = this.generation; this.lastList.set(deviceId, Date.now());
    const [workspaces, threads] = await this.withSignal(signal => Promise.all([
      this.io.get<WorkspaceDto[]>(deviceId, 'api/workspaces', { signal, cache: 'no-store' }),
      this.io.get<ThreadDto[]>(deviceId, 'api/threads?includeAgentThreads=true', { signal, cache: 'no-store' }),
    ]));
    if (this.stopped || generation !== this.generation || !Array.isArray(workspaces) || !Array.isArray(threads)) return;
    const previous = new Map(this.state.threads.map(row => [row.key, row]));
    const next = threads.map(row => {
      const old = previous.get(entityKey(deviceId, row.id));
      return old && old.updatedAt > row.updatedAt ? old : summarizeThread(deviceId, row, old);
    });
    this.publish({
      workspaces: mergeRows(this.state.workspaces, [...this.state.workspaces.filter(row => row.deviceId !== deviceId), ...workspaces.map(row => ({ ...row, key: entityKey(deviceId, row.id), deviceId }))], row => row.key),
      threads: mergeRows(this.state.threads, [...this.state.threads.filter(row => row.deviceId !== deviceId), ...next], row => row.key),
      devices: mergeRows(this.state.devices, this.state.devices.map(row => row.id === deviceId ? { ...row, cachedAt: new Date().toISOString() } : row), row => row.id),
    });
    const pending = next.filter(row => !row.closedAt && (!this.requestHydrated.has(row.key) || previous.get(row.key)?.updatedAt !== row.updatedAt));
    let cursor = 0;
    // Initial request recovery is bounded and runs once per known revision; it
    // is not a recurring summary poll for every idle thread.
    await Promise.all(Array.from({ length: Math.min(6, pending.length) }, async () => {
      while (cursor < pending.length && visible() && !this.stopped && generation === this.generation) {
        const row = pending[cursor++]!;
        try { await this.loadDetail(row.deviceId, row.id, true); this.requestHydrated.add(row.key); } catch { /* Preserve the cached request on a lost read. */ }
      }
    }));
  });
  refreshActivity = (deviceId: string, allowOffMap = false): Promise<void> => this.single(`activity:${deviceId}`, async () => {
    if ((!this.mapVisible && !allowOffMap) || !visible() || !this.online(deviceId) || this.stopped) return;
    const generation = this.generation; this.lastActivity.set(deviceId, Date.now()); this.dirtyActivity.delete(deviceId);
    this.extra(deviceId, { syncing: true });
    try {
      const activity = await this.withSignal(signal => this.io.get<ThreadActivityResponseDto>(deviceId, 'api/thread-activity?hours=24', { signal, cache: 'no-store' }));
      if (this.stopped || generation !== this.generation || !Array.isArray(activity.threads)) return;
      const previous = this.state.activity[deviceId];
      const [stable] = mergeRows(previous ? [previous] : [], [activity], () => deviceId);
      if (stable !== previous) this.publish({ activity: { ...this.state.activity, [deviceId]: stable! } });
      if (this.mapVisible) void this.withSignal(signal => this.io.get<{ totals?: { costUsd?: number | null } }>(deviceId, `api/stats/summary?range=today&tz=${encodeURIComponent(Intl.DateTimeFormat().resolvedOptions().timeZone)}`, { signal, cache: 'no-store' })).then(summary => {
        if (generation === this.generation && !this.stopped && typeof summary.totals?.costUsd === 'number') this.extra(deviceId, { todayCostUsd: summary.totals.costUsd });
      }).catch(() => undefined);
    } catch { /* Older Supervisors may not support activity; cached values remain. */ }
    finally { if (generation === this.generation) this.extra(deviceId, { syncing: false }); }
  });
  refreshExtras = (deviceId: string): Promise<void> => this.single(`extras:${deviceId}`, async () => {
    if (!this.mapVisible || !visible() || !this.online(deviceId) || this.stopped) return;
    const generation = this.generation; const started = performance.now(); this.lastExtra.set(deviceId, Date.now());
    await Promise.allSettled([
      this.withSignal(signal => this.io.get<DeviceMetricsDto>(deviceId, 'api/device/metrics', { signal, cache: 'no-store' })).then(metrics => { if (generation === this.generation && !this.stopped) this.extra(deviceId, { metrics, latencyMs: Math.round(performance.now() - started) }); }),
      ...(Date.now() - (this.lastRuntime.get(deviceId) ?? 0) < 60_000 ? [] : [this.io.get<{ runningVersion?: string; latestVersion?: string }>(deviceId, 'api/management/supervisor', { cache: 'no-store' }).then(runtime => {
        if (generation !== this.generation || this.stopped) return;
        this.lastRuntime.set(deviceId, Date.now());
        this.extra(deviceId, { ...(runtime.runningVersion ? { runtimeVersion: runtime.runningVersion } : {}), ...(runtime.latestVersion ? { latestVersion: runtime.latestVersion, updateAvailable: runtime.latestVersion !== runtime.runningVersion } : {}) });
      })]),
    ]);
  });
  private extra(deviceId: string, patch: Partial<DesktopExtra>) {
    const next = { ...this.state.extras[deviceId], ...patch };
    const [stable] = mergeRows(this.state.extras[deviceId] ? [this.state.extras[deviceId]!] : [], [next], () => deviceId);
    if (stable !== this.state.extras[deviceId]) this.publish({ extras: { ...this.state.extras, [deviceId]: stable! } });
  }
  private single(key: string, task: () => Promise<void>): Promise<void> {
    const pending = this.requests.get(key); if (pending) return pending;
    const promise = task().catch(() => undefined).finally(() => { this.requests.delete(key); });
    this.requests.set(key, promise); return promise;
  }
  private updateThread(key: string, update: (thread: DesktopThread) => DesktopThread) {
    this.publish({ threads: mergeRows(this.state.threads, this.state.threads.map(row => row.key === key ? update(row) : row), row => row.key) });
  }
  markRead = (key: string) => this.updateThread(key, row => ({ ...row, lastViewedAt: new Date().toISOString() }));
  patchThread = (deviceId: string, thread: ThreadDto) => this.updateThread(entityKey(deviceId, thread.id), row => summarizeThread(deviceId, thread, row));
  acceptDetail(deviceId: string, detail: ThreadDetailDto, requestsAtStart?: DesktopThread['requests']) {
    const key = entityKey(deviceId, detail.thread.id);
    this.updateThread(key, row => ({ ...(detail.thread.updatedAt >= row.updatedAt ? summarizeThread(deviceId, detail.thread, row) : row),
      lastTurnId: detail.turns.at(-1)?.id ?? row.lastTurnId,
      requests: requestsAtStart === undefined || row.requests === requestsAtStart ? detail.pendingRequests.map(summarizeRequest) : row.requests,
    }));
  }
  async loadDetail(deviceId: string, threadId: string, force = false): Promise<ThreadDetailDto> {
    const key = entityKey(deviceId, threadId), pending = this.detailRequests.get(key); if (pending) return pending;
    const cached = this.details.get(key); if (!force && cached && Date.now() - cached.at < 15_000) return cached.detail;
    const generation = this.generation, requestState = this.state.threads.find(row => row.key === key)?.requests;
    const task = this.withSignal(signal => this.io.get<ThreadDetailDto>(deviceId, `api/threads/${encodeURIComponent(threadId)}?view=summary&limit=1&compact=1`, { signal, cache: 'no-store' })).then(detail => {
      if (generation !== this.generation || this.stopped) return detail;
      this.details.delete(key); this.details.set(key, { detail, at: Date.now() });
      while (this.details.size > 8) this.details.delete(this.details.keys().next().value!);
      this.acceptDetail(deviceId, detail, requestState); return detail;
    }).finally(() => this.detailRequests.delete(key));
    this.detailRequests.set(key, task); return task;
  }
  private scheduleList(deviceId: string) {
    if (this.pendingLists.has(deviceId)) return;
    const delay = Math.max(0, 2000 - (Date.now() - (this.lastList.get(deviceId) ?? 0)));
    this.pendingLists.set(deviceId, setTimeout(() => { this.pendingLists.delete(deviceId); void this.refreshDevice(deviceId); }, delay));
  }
  private scheduleDetail(deviceId: string, threadId: string) {
    const key = entityKey(deviceId, threadId); this.dirtyDetails.add(key);
    clearTimeout(this.detailTimers.get(key));
    if (!visible()) return;
    this.detailTimers.set(key, setTimeout(() => {
      this.detailTimers.delete(key); if (!visible() || this.stopped) return;
      this.dirtyDetails.delete(key); void this.loadDetail(deviceId, threadId, true).catch(() => undefined);
    }, 2000));
  }
  applyEvent(deviceId: string, event: ThreadEventEnvelope) {
    if (!isIndexEvent(event.type) || this.stopped) return;
    const key = entityKey(deviceId, event.threadId);
    if (!this.state.threads.some(row => row.key === key)) { this.scheduleList(deviceId); return; }
    this.updateThread(key, row => {
      if (event.timestamp < row.updatedAt) return row;
      switch (event.type) {
        case 'thread.updated': {
          const status = event.payload.status;
          const allowed = ['running', 'recovering', 'idle', 'interrupted', 'failed', 'not_loaded', 'system_error'];
          return { ...row, title: event.payload.title ?? row.title, status: status && allowed.includes(status) ? status as DesktopThread['status'] : row.status,
            closedAt: event.payload.reason === 'closed' ? event.timestamp : row.closedAt ?? null,
            updatedAt: event.payload.reason ? row.updatedAt : event.timestamp };
        }
        case 'thread.turn.started': return { ...row, resultSummary: null, status: 'running', activeTurnId: event.payload.turnId, lastTurnId: event.payload.turnId, lastTurnStartedAt: event.timestamp, closedAt: null, updatedAt: event.timestamp };
        case 'thread.turn.completed': return { ...row, status: event.payload.status === 'failed' ? 'failed' : event.payload.status === 'interrupted' ? 'interrupted' : 'idle', activeTurnId: null, lastTurnId: event.payload.turnId, lastTurnCompletedAt: event.timestamp, updatedAt: event.timestamp };
        case 'thread.turn.failed': return { ...row, status: event.payload.willRetry ? 'recovering' : 'failed', activeTurnId: event.payload.willRetry ? row.activeTurnId : null, lastTurnId: event.payload.turnId, lastTurnCompletedAt: event.payload.willRetry ? row.lastTurnCompletedAt : event.timestamp, resultSummary: plainSummary(event.payload.error), updatedAt: event.timestamp };
        case 'thread.request.created': return { ...row, requests: [...row.requests.filter(request => request.id !== event.payload.request.id), summarizeRequest(event.payload.request)], updatedAt: event.timestamp };
        case 'thread.request.resolved': return { ...row, requests: row.requests.filter(request => request.id !== event.payload.requestId), updatedAt: event.timestamp };
        default: return row;
      }
    });
    if (event.type === 'thread.updated' && event.payload.reason) this.scheduleList(deviceId);
    if (event.type === 'thread.turn.completed' || event.type === 'thread.turn.failed') {
      this.dirtyActivity.add(deviceId);
      this.scheduleDetail(deviceId, event.threadId);
      clearTimeout(this.activityTimers.get(deviceId));
      if (this.mapVisible && visible()) this.activityTimers.set(deviceId, setTimeout(() => { this.activityTimers.delete(deviceId); void this.refreshActivity(deviceId); }, 2000));
    }
    if (this.mapVisible && visible() && ['thread.request.created', 'thread.turn.completed', 'thread.turn.failed'].includes(event.type)) {
      if (this.state.pulseKeys.includes(key)) {
        this.publish({ pulseKeys: this.state.pulseKeys.filter(value => value !== key) });
        requestAnimationFrame(() => { if (!this.stopped && this.mapVisible && visible()) this.publish({ pulseKeys: [...new Set([...this.state.pulseKeys, key])] }); });
      } else this.publish({ pulseKeys: [...this.state.pulseKeys, key] });
      clearTimeout(this.pulseTimer); this.pulseTimer = setTimeout(() => this.publish({ pulseKeys: [] }), 1000);
    }
  }
  private connect(deviceId: string, attempt = 0) {
    if (this.stopped || this.sockets.has(deviceId) || !this.online(deviceId)) return;
    const socket = this.io.socket(deviceId, event => { if (isIndexEvent(event.type)) this.applyEvent(deviceId, event); });
    const heartbeat = new SupervisorHeartbeat(); heartbeat.probe(Date.now()); this.sockets.set(deviceId, { socket, heartbeat });
    socket.addEventListener('message', () => heartbeat.receive(Date.now()));
    socket.addEventListener('open', () => { attempt = 0; heartbeat.probe(Date.now()); this.scheduleList(deviceId); });
    socket.addEventListener('close', () => {
      if (this.sockets.get(deviceId)?.socket !== socket) return;
      this.sockets.delete(deviceId); if (this.stopped || !this.online(deviceId)) return;
      const delay = [1000, 2000, 5000, 15000][Math.min(attempt, 3)]!;
      this.retries.set(deviceId, setTimeout(() => { this.retries.delete(deviceId); this.connect(deviceId, attempt + 1); }, delay));
    });
  }
}
