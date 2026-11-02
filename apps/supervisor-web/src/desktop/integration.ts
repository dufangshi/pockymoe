import { translate } from '@pockymoe/thread-ui/i18n';
import type { HostCommand, HostCommandResult, PockymoeHost } from './host';
import { deviceRequest, respondToThreadRequest, sendThreadPrompt } from '../lib/api';
import { buildActivitySnapshot } from './data/activity';
import type { DesktopIndexStore } from './data/store';
import { newerVersion } from './devices/model';

/** The first Supervisor with the one-off completion route that spoken summaries run through. */
const COMPLETION_VERSION = '0.13.43';

export function focusedKeyFromPath(path: string): string | null {
  const match = path.match(/^\/devices\/([^/]+)\/threads\/([^/]+)$/);
  return match && !['new', 'import'].includes(match[2]!) ? `${decodeURIComponent(match[1]!)}:${decodeURIComponent(match[2]!)}` : null;
}
export interface CommandDependencies {
  store: DesktopIndexStore; navigate(path: string): void; openMap(): void; revealInMap(key: string): void; newThread(): void; openUpstreams?(): void;
  respond?: typeof respondToThreadRequest; send?: typeof sendThreadPrompt; request?: typeof deviceRequest;
}
export class NeedsThreadError extends Error { readonly code = 'needsThread'; }
export async function resolveRequest(deviceId: string, threadId: string, requestId: string, decision: 'approve' | 'deny', deps: Pick<CommandDependencies, 'store' | 'respond'>) {
  const detail = await deps.store.loadRequestDetail(deviceId, threadId);
  const action = detail.pendingRequests.find(request => request.id === requestId);
  if (!action) throw new Error(translate('desktopShell.requestResolved'));
  if (action.kind === 'permissionRequest' && action.questions.length === 0) {
    const updated = await (deps.respond ?? respondToThreadRequest)(threadId, requestId, { allow: decision === 'approve' }, deviceId);
    deps.store.acceptDetail(deviceId, updated); return;
  }
  const needsThread = () => new NeedsThreadError(translate(action.kind === 'requestUserInput' ? 'desktopShell.inputNeedsThread' : 'desktopShell.optionsNeedThread'));
  const question = action.questions[0];
  if (!question || action.questions.length !== 1 || question.multiSelect || question.isOther) throw needsThread();
  const options = question.options ?? [];
  const approvePattern = /^(?:(?:allow|approve|accept|yes)(?:\b|\s|$)|允许|批准|同意)/i;
  const denyPattern = /^(?:(?:deny|reject|decline|no)(?:\b|\s|$)|拒绝|不允许)/i;
  const matches = (option: typeof options[number], pattern: RegExp) => pattern.test(option.description ?? '') || pattern.test(option.label);
  if (action.kind === 'requestUserInput' && (options.length !== 2 || !options.some(option => matches(option, approvePattern)) || !options.some(option => matches(option, denyPattern)))) throw needsThread();
  const pattern = action.kind === 'planDecision' ? decision === 'approve' ? /^(implement|proceed|执行|实施)(?:\b|\s|$)/i : /^(keep planning|reject|cancel|保持规划|继续规划)(?:\b|\s|$)/i : decision === 'approve' ? approvePattern : denyPattern;
  const once = decision === 'approve' ? options.find(option => /allow.once|approve.once/i.test(option.description ?? '')) : undefined;
  const option = once ?? options.find(option => pattern.test(option.description)) ?? options.find(option => pattern.test(option.label));
  if (!option) throw needsThread();
  const updated = await (deps.respond ?? respondToThreadRequest)(threadId, requestId, { answers: { [question.id]: { answers: [option.label] } } }, deviceId);
  deps.store.acceptDetail(deviceId, updated);
}
interface TextContextMessage { role: 'user' | 'assistant'; text: string; turnId?: string }
/** The turn's request and final answer. The two newest turns cover one that started right after this one ended. */
export async function turnText(deviceId: string, threadId: string, turnId: string | null | undefined, deps: Pick<CommandDependencies, 'request'>) {
  const request = deps.request ?? deviceRequest;
  const { messages } = await request<{ messages: TextContextMessage[] }>(deviceId, `api/threads/${encodeURIComponent(threadId)}/summary?turns=2&maxChars=20000`);
  const owner = turnId && messages.some(message => message.turnId === turnId) ? turnId : messages.at(-1)?.turnId;
  const own = messages.filter(message => message.turnId === owner);
  return { request: own.find(message => message.role === 'user')?.text ?? '', answer: own.filter(message => message.role === 'assistant').at(-1)?.text ?? '' };
}
interface SavedConnection { id: string; name: string; protocol: string; model?: string; local?: boolean }
/** A device that cannot answer is skipped: the others' connections are still usable. */
export async function listConnections(deps: Pick<CommandDependencies, 'store' | 'request'>) {
  const request = deps.request ?? deviceRequest;
  const devices = deps.store.getSnapshot().devices.filter(device => device.connected);
  const lists = await Promise.all(devices.map(async device => {
    try {
      const [{ connections }, status] = await Promise.all([
        request<{ connections: SavedConnection[] }>(device.id, 'api/management/connections'),
        request<{ runningVersion?: string }>(device.id, 'api/management/supervisor').catch(() => null),
      ]);
      // A summary runs on the device holding the upstream, so its version decides whether this one can be used.
      const version = status?.runningVersion;
      return connections.filter(connection => connection.local !== false).map(connection => ({ deviceId: device.id, deviceName: device.name, id: connection.id, name: connection.name, protocol: connection.protocol, model: connection.model ?? '', ...(version ? { supervisorVersion: version, needsUpdate: newerVersion(version, COMPLETION_VERSION) } : {}) }));
    } catch { return []; }
  }));
  return lists.flat();
}
/** What can be silenced, newest conversation first. An agent thread belongs to its conversation, so only roots are listed. */
export function muteTargets(store: Pick<DesktopIndexStore, 'getSnapshot'>) {
  const index = store.getSnapshot();
  const devices = new Map(index.devices.map(device => [device.id, device.name]));
  const workspaces = new Map(index.workspaces.map(workspace => [workspace.key, workspace.label]));
  const recent = (thread: typeof index.threads[number]) => thread.lastTurnStartedAt ?? thread.updatedAt ?? '';
  return {
    devices: index.devices.map(device => ({ id: device.id, label: device.name })),
    workspaces: index.workspaces.map(workspace => ({ id: workspace.key, label: `${workspace.label} · ${devices.get(workspace.deviceId) ?? ''}` })),
    threads: index.threads.filter(thread => !thread.rootThreadId || thread.rootThreadId === thread.id)
      .sort((a, b) => recent(b).localeCompare(recent(a))).slice(0, 500)
      .map(thread => ({ id: thread.key, label: `${thread.title} · ${workspaces.get(`${thread.deviceId}:${thread.workspaceId}`) ?? devices.get(thread.deviceId) ?? ''}` })),
  };
}
export async function handleHostCommand(command: HostCommand, deps: CommandDependencies): Promise<HostCommandResult> {
  try {
    switch (command.type) {
      case 'navigate':
        if (!command.path.startsWith('/') || command.path.startsWith('//') || command.path.includes('\\')) throw new Error(translate('desktopShell.invalidPath'));
        deps.navigate(command.path); break;
      case 'markRead': deps.store.markRead(command.key); break;
      case 'resolveRequest': await resolveRequest(command.deviceId, command.threadId, command.requestId, command.decision, deps); break;
      case 'reply': {
        if (!command.text.trim()) throw new Error(translate('desktopShell.emptyReply'));
        const thread = await (deps.send ?? sendThreadPrompt)(command.threadId, { prompt: command.text, clientRequestId: command.id }, command.deviceId);
        deps.store.patchThread(command.deviceId, thread); break;
      }
      case 'revealInMap': deps.revealInMap(command.key); break;
      case 'openMap': deps.openMap(); break;
      case 'newThread': deps.newThread(); break;
      case 'openDevices': deps.navigate('/desktop/devices'); break;
      case 'turnText': return { ok: true, data: await turnText(command.deviceId, command.threadId, command.turnId, deps) };
      case 'listConnections': return { ok: true, data: await listConnections(deps) };
      case 'openUpstreams': deps.openUpstreams?.(); break;
      case 'listMuteTargets': return { ok: true, data: muteTargets(deps.store) };
      case 'connectionModels': {
        const { models } = await (deps.request ?? deviceRequest)<{ models: { id: string; name?: string }[] }>(command.deviceId, `api/management/connections/${encodeURIComponent(command.connectionId)}/models`, { method: 'POST', body: '{}' });
        return { ok: true, data: models.map(model => ({ id: model.id, name: model.name ?? model.id })) };
      }
      case 'completeWithConnection': {
        const request = deps.request ?? deviceRequest;
        try {
          const { text } = await request<{ text: string }>(command.deviceId, `api/management/connections/${encodeURIComponent(command.connectionId)}/complete`, { method: 'POST', body: JSON.stringify({ model: command.model, system: command.system, user: command.user }) });
          return { ok: true, data: { text } };
        } catch (error) {
          // An older Supervisor does not have this route yet.
          if ([404, 405].includes((error as { statusCode?: number }).statusCode ?? 0)) return { ok: false, code: 'supervisorTooOld', error: translate('settings.connUpdateSupervisor') };
          throw error;
        }
      }
    }
    return { ok: true };
  } catch (error) { return { ok: false, error: error instanceof Error ? error.message : translate('desktopShell.actionFailed'), ...(error instanceof NeedsThreadError ? { code: error.code } : {}) }; }
}
let activitySession = 0;
/** Change-only snapshots, never timer ticks, <=4/s. */
export function syncActivity(store: DesktopIndexStore, host: PockymoeHost, focused: () => string | null, badgeEnabled: boolean) {
  const session = ++activitySession; let logoutPending = false;
  let previous = ''; let lastSent = 0; let timer: ReturnType<typeof setTimeout> | undefined;
  const flush = () => {
    timer = undefined;
    if (session !== activitySession) return;
    const snapshot = buildActivitySnapshot(store.getSnapshot(), logoutPending ? null : focused(), logoutPending ? false : document.hasFocus());
    const signature = JSON.stringify({ ...snapshot, generatedAt: '' });
    if (signature === previous) return;
    previous = signature; lastSent = Date.now(); host.activity?.sync(snapshot);
    const waiting = new Set(snapshot.items.filter(item => item.state === 'needsInput').map(item => item.rootThreadKey ?? item.key)).size;
    const active = new Set(snapshot.items.filter(item => item.state === 'running').map(item => item.rootThreadKey ?? item.key)).size;
    if (badgeEnabled && typeof host.badge?.set === 'function') host.badge.set(waiting || active, waiting ? 'attention' : active ? 'running' : 'none');
  };
  const schedule = () => { timer ??= setTimeout(flush, Math.max(0, 250 - (Date.now() - lastSent))); };
  const unsubscribe = store.subscribe(schedule);
  const logout = () => { logoutPending = true; schedule(); };
  window.addEventListener('pockymoe-account-logout', logout);
  window.addEventListener('focus', schedule); window.addEventListener('blur', schedule); window.addEventListener('pockymoe-desktop-route', schedule);
  const age = setInterval(schedule, 30_000); schedule();
  return () => { unsubscribe(); if (!logoutPending) clearTimeout(timer); clearInterval(age); window.removeEventListener('pockymoe-account-logout', logout); window.removeEventListener('focus', schedule); window.removeEventListener('blur', schedule); window.removeEventListener('pockymoe-desktop-route', schedule); };
}
