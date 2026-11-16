import React from 'react';
import { ThreadShellPanel, ThreadTimeline, type ThreadShellAdapter } from '@pockymoe/thread-ui';
import type { ShellSessionDto, ThreadDetailDto } from '@pockymoe/shared';

const shell: ShellSessionDto = { id: 'audit-shell', threadId: 'audit', workspaceId: 'audit', label: 'Audit terminal', tmuxSessionName: 'audit', backend: 'pty', cwd: '/audit', status: 'running', attachedViewerId: null, createdAt: '2026-11-15T00:00:00Z', updatedAt: '2026-11-15T00:00:00Z', lastActivityAt: null };
const shellState = { threadId: 'audit', workspaceId: 'audit', workspacePathStatus: 'present' as const, state: 'running' as const, shell, shells: [shell], activeShellId: shell.id };
const shellAdapter: ThreadShellAdapter = {
  fetchState: async () => shellState, createShell: async () => shellState,
  terminateShell: async () => shell, updateShell: async () => shell,
  connectSocket(handlers) {
    const socket = new EventTarget() as WebSocket;
    Object.defineProperty(socket, 'readyState', { value: WebSocket.OPEN });
    socket.close = () => {};
    let attached = false;
    const output = (data: string) => handlers.onShellEvent?.({ type: 'shell.output', shellId: shell.id, timestamp: new Date().toISOString(), payload: { data } });
    Object.assign(window, { vendorWrite: output, vendorInput: '' });
    queueMicrotask(() => handlers.onConnected?.({}));
    return { socket, close() {}, send(message) {
      const value = message as { type: string; data?: string };
      if (value.type === 'shell.attach' && !attached) { attached = true; queueMicrotask(() => {
        handlers.onShellEvent?.({ type: 'shell.connected', shellId: shell.id, timestamp: new Date().toISOString(), payload: { viewerId: 'audit-viewer' } });
        output('Pockymoe terminal audit\r\n$ ');
      }); }
      if (value.type === 'shell.input') {
        const page = window as unknown as { vendorInput: string };
        page.vendorInput += value.data ?? ''; output(value.data ?? '');
      }
    } };
  },
};
const diagram = ['```mermaid', 'flowchart LR', 'A[Source] e1@--> B[Target]', 'e1@{ animate: true, animation: fast }', '```'].join('\n');
const detail = { thread: { id: 'audit', title: 'Library animation audit', status: 'idle' }, workspace: { id: 'audit', label: 'Audit workspace', absPath: '/audit' }, workspacePathStatus: 'present', turns: [{ id: 'audit-turn', status: 'completed', startedAt: '2026-11-15T00:00:00Z', completedAt: '2026-11-15T00:01:00Z', error: null, items: [{ id: 'audit-message', kind: 'agentMessage', text: diagram }] }], pendingRequests: [], pendingSteers: [] } as unknown as ThreadDetailDto;

export function BuiltLibraries() {
  const params = new URLSearchParams(location.search);
  const mode = params.get('libraries');
  const dark = params.get('mode') === 'dark';
  const turns = mode === 'mermaid-custom' ? [{ ...detail.turns[0]!, items: [{ id: 'custom-diagram', kind: 'agentMessage' as const, text: diagram.replace('e1@{ animate: true, animation: fast }', 'e1@{ animate: true, animation: fast }\nclassDef moving animation:dash 1s linear infinite;\nclass A moving;') }] }] : detail.turns;
  const terminal = <ThreadShellPanel threadId="audit" shellAdapter={shellAdapter} isVisible showHeader effectiveTheme={dark ? 'dark' : 'light'} />;
  const timeline = <ThreadTimeline turns={turns} liveOutput="" />;
  return <main className="library-audit" style={{ height: 600, padding: 20, background: 'var(--theme-bg)', color: 'var(--theme-fg)' }}>
    <button id="vendor-blur">Focus outside the terminal</button>
    {mode === 'terminal' && terminal}
    {(mode === 'mermaid' || mode === 'mermaid-custom') && timeline}
  </main>;
}
