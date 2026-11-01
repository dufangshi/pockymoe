import type { HostCommand, HostCommandResult, PockymoeHost } from './host';
import { respondToThreadRequest, sendThreadPrompt } from '../lib/api';
import { buildActivitySnapshot } from './data/activity';
import type { DesktopIndexStore } from './data/store';

export function focusedKeyFromPath(path: string): string | null {
  const match = path.match(/^\/devices\/([^/]+)\/threads\/([^/]+)$/);
  return match && !['new', 'import'].includes(match[2]!) ? `${decodeURIComponent(match[1]!)}:${decodeURIComponent(match[2]!)}` : null;
}
export interface CommandDependencies {
  store: DesktopIndexStore; navigate(path: string): void; openMap(): void; revealInMap(key: string): void; newThread(): void;
  respond?: typeof respondToThreadRequest; send?: typeof sendThreadPrompt;
}
export async function resolveRequest(deviceId: string, threadId: string, requestId: string, decision: 'approve' | 'deny', deps: Pick<CommandDependencies, 'store' | 'respond'>) {
  const detail = await deps.store.loadDetail(deviceId, threadId, true);
  const action = detail.pendingRequests.find(request => request.id === requestId);
  if (!action) throw new Error('请求已处理。');
  if (action.kind === 'requestUserInput') throw new Error('请打开线程，回答这个输入请求。');
  const question = action.questions[0];
  if (!question || action.questions.length !== 1) throw new Error('请在线程中检查这个请求。');
  const options = question.options ?? [];
  const pattern = decision === 'approve' ? /^(allow|approve|accept|yes|允许|批准|同意)(?:\b|\s|$)/i : /^(deny|reject|decline|no|拒绝|不允许)(?:\b|\s|$)/i;
  const once = decision === 'approve' ? options.find(option => /allow.once|approve.once/i.test(option.description ?? '')) : undefined;
  const option = once ?? options.find(option => pattern.test(option.description)) ?? options.find(option => pattern.test(option.label));
  if (!option) throw new Error('请在线程中检查可用的审批选项。');
  const updated = await (deps.respond ?? respondToThreadRequest)(threadId, requestId, { answers: { [question.id]: { answers: [option.label] } } }, deviceId);
  deps.store.acceptDetail(deviceId, updated);
}
export async function handleHostCommand(command: HostCommand, deps: CommandDependencies): Promise<HostCommandResult> {
  try {
    switch (command.type) {
      case 'navigate':
        if (!command.path.startsWith('/') || command.path.startsWith('//') || command.path.includes('\\')) throw new Error('无法打开这个应用路径。');
        deps.navigate(command.path); break;
      case 'markRead': deps.store.markRead(command.key); break;
      case 'resolveRequest': await resolveRequest(command.deviceId, command.threadId, command.requestId, command.decision, deps); break;
      case 'reply': {
        if (!command.text.trim()) throw new Error('回复不能为空。');
        const thread = await (deps.send ?? sendThreadPrompt)(command.threadId, { prompt: command.text, clientRequestId: command.id }, command.deviceId);
        deps.store.patchThread(command.deviceId, thread); break;
      }
      case 'revealInMap': deps.revealInMap(command.key); break;
      case 'openMap': deps.openMap(); break;
      case 'newThread': deps.newThread(); break;
      case 'openDevices': deps.navigate('/desktop/devices'); break;
    }
    return { ok: true };
  } catch (error) { return { ok: false, error: error instanceof Error ? error.message : '操作没有完成，请重试。' }; }
}
/** Change-only snapshots, never timer ticks, <=4/s. */
export function syncActivity(store: DesktopIndexStore, host: PockymoeHost, focused: () => string | null, badgeEnabled: boolean) {
  let previous = ''; let lastSent = 0; let timer: ReturnType<typeof setTimeout> | undefined;
  const flush = () => {
    timer = undefined;
    const snapshot = buildActivitySnapshot(store.getSnapshot(), focused(), document.hasFocus());
    const signature = JSON.stringify({ ...snapshot, generatedAt: '' });
    if (signature === previous) return;
    previous = signature; lastSent = Date.now(); host.activity?.sync(snapshot);
    const waiting = new Set(snapshot.items.filter(item => item.state === 'needsInput').map(item => item.rootThreadKey ?? item.key)).size;
    const active = new Set(snapshot.items.filter(item => item.state === 'running').map(item => item.rootThreadKey ?? item.key)).size;
    if (badgeEnabled && typeof host.badge?.set === 'function') host.badge.set(waiting || active, waiting ? 'attention' : active ? 'running' : 'none');
  };
  const schedule = () => { timer ??= setTimeout(flush, Math.max(0, 250 - (Date.now() - lastSent))); };
  const unsubscribe = store.subscribe(schedule);
  window.addEventListener('focus', schedule); window.addEventListener('blur', schedule); window.addEventListener('pockymoe-desktop-route', schedule);
  const age = setInterval(schedule, 30_000); schedule();
  return () => { unsubscribe(); clearTimeout(timer); clearInterval(age); window.removeEventListener('focus', schedule); window.removeEventListener('blur', schedule); window.removeEventListener('pockymoe-desktop-route', schedule); };
}
