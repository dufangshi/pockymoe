import { createRoot } from 'react-dom/client';
import { ThreadShellPanel } from '@pockymoe/thread-ui';
import type { ThreadShellAdapter } from '@pockymoe/thread-ui';
import '@pockymoe/thread-ui/styles.css';
import '../src/index.css';
import { installTouchTapGuard } from '../src/lib/touchTapGuard';
import { applyTheme } from '../src/lib/themePacks';
import { setLocale } from '@pockymoe/thread-ui/i18n';

installTouchTapGuard();
setLocale('en');
const parameters = new URLSearchParams(location.search);
const mode = parameters.get('mode') === 'dark' ? 'dark' : 'light';
document.documentElement.dataset.themeEffective = mode;
document.documentElement.classList.toggle('dark', mode === 'dark');
await applyTheme(parameters.get('pack') ?? 'classic');
const shell = {
  id: 'terminal-fixture', threadId: 'thread-fixture', workspaceId: 'workspace-fixture',
  label: null, tmuxSessionName: 'fixture', backend: 'pty' as const, cwd: '/tmp', status: 'running' as const,
  attachedViewerId: null, createdAt: '2026-11-02T00:00:00Z', updatedAt: '2026-11-02T00:00:00Z', lastActivityAt: null,
};
const state = { threadId: shell.threadId, workspaceId: shell.workspaceId, state: 'running' as const, shell, shells: [shell], activeShellId: shell.id, workspacePathStatus: 'present' as const };
const adapter: ThreadShellAdapter = {
  fetchState: async () => state,
  createShell: async () => state,
  terminateShell: async () => shell,
  updateShell: async () => shell,
  connectSocket(handlers) {
    const socket = new EventTarget() as WebSocket;
    Object.defineProperty(socket, 'readyState', { value: WebSocket.OPEN });
    const emit = (type: string, payload: unknown) => handlers.onShellEvent?.({ type, shellId: shell.id, timestamp: shell.createdAt, payload } as never);
    let attached = false;
    const connection = { socket, close() {}, send(message: unknown) {
      if ((message as { type: string }).type !== 'shell.attach' || attached) return;
      attached = true;
      emit('shell.connected', { viewerId: 'fixture-viewer' });
      emit('shell.output', { data: '\x1b[2J\x1b[HClipboardMarkerAlpha\r\n\x1b]8;;https://example.com/terminal\x1b\\TerminalLink\x1b]8;;\x1b\\\r\n$ ', replay: true, replace: true });
    } };
    requestAnimationFrame(() => requestAnimationFrame(() => handlers.onConnected?.({})));
    return connection;
  },
};
createRoot(document.getElementById('root')!).render(<div className="thread-ui-shell" style={{ height: '100vh' }}><ThreadShellPanel threadId={shell.threadId} shellAdapter={adapter} effectiveTheme={mode} isVisible /></div>);
