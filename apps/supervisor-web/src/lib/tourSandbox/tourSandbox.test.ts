import { setLocale } from '@pockymoe/thread-ui/i18n';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { connectTourSocket, handleTourRequest, resetTourSandbox } from './server';
import { TourShell } from './shell';
import { isTourRequest, TOUR_HELPER_THREAD, TOUR_MAIN_THREAD } from './tourDevice';
import { DEMO_WORKSPACE } from './sample';

const api = (path: string) => `relay/devices/pockymoe-tour/api/${path}`;
const read = async <T>(path: string, init: RequestInit = {}) => (await (await handleTourRequest(path, init)).json()) as T;
const post = (body: unknown, method = 'POST') => ({ method, body: JSON.stringify(body) });

beforeEach(() => {
  setLocale('en', false);
  resetTourSandbox();
});
afterEach(() => vi.useRealTimers());

describe('tour sample device', () => {
  it('answers only its own requests, and relay calls only from its pages', () => {
    expect(isTourRequest(api('workspaces'))).toBe(true);
    expect(isTourRequest('relay/devices/laptop/api/workspaces')).toBe(false);
    expect(isTourRequest('relay/portal')).toBe(false);
    window.history.pushState(null, '', `/devices/pockymoe-tour/threads/${TOUR_MAIN_THREAD}`);
    expect(isTourRequest('relay/portal')).toBe(true);
    expect(isTourRequest('relay/account/workbench')).toBe(false);
    window.history.pushState(null, '', '/');
  });

  it('serves the sample workspace, its threads and the helper agent under its parent', async () => {
    const workspaces = await read<Array<{ id: string; label: string }>>(api('workspaces'));
    expect(workspaces.map((workspace) => workspace.label)).toEqual(['pockymoe-demo', 'docs-site']);
    const roots = await read<Array<{ id: string }>>(api('threads'));
    expect(roots.map((thread) => thread.id)).not.toContain(TOUR_HELPER_THREAD);
    const all = await read<Array<{ id: string; parentThreadId?: string }>>(api('threads?includeAgentThreads=true'));
    expect(all.find((thread) => thread.id === TOUR_HELPER_THREAD)?.parentThreadId).toBe(TOUR_MAIN_THREAD);
    const detail = await read<{ turns: Array<{ items: Array<{ peerWork?: { taskTurnId: string } }> }> }>(api(`threads/${TOUR_MAIN_THREAD}?view=summary&limit=3`));
    expect(detail.turns[1]!.items[0]!.peerWork?.taskTurnId).toBe('tour-turn-task');
    const helper = await read<{ turns: Array<{ items: Array<{ text: string }> }> }>(api(`threads/${TOUR_HELPER_THREAD}`));
    expect(helper.turns[0]!.items[0]!.text).toMatch(new RegExp(`^\\[Pockymoe task from thread ${TOUR_MAIN_THREAD} \\|`));
  });

  it('saves a file only over the version it was read from', async () => {
    const path = api(`workspaces/${DEMO_WORKSPACE}/files`);
    const document = await read<{ contentHash: string }>(`${path}/document?path=docs%2Fnotes.md`);
    const save = (expectedHash: string, content: string, operationId: string) =>
      read<{ status: string }>(`${path}/save`, post({ path: 'docs/notes.md', expectedHash, content, draftRevision: 1, operationId }));
    expect((await save(document.contentHash, 'mine\n', 'a')).status).toBe('saved');
    expect((await save(document.contentHash, 'stale\n', 'b')).status).toBe('conflict');
    expect((await read<{ content: string }>(`${path}/preview?path=docs%2Fnotes.md`)).content).toBe('mine\n');
    await read(path, post({ path: 'src/new.ts' }));
    const tree = await read<{ children: Array<{ name: string }> }>(`${path}/tree?path=src`);
    expect(tree.children.map((node) => node.name)).toContain('new.ts');
  });

  it('streams a scripted reply over its socket and stops it on request', async () => {
    vi.useFakeTimers();
    const events: Array<{ type: string; payload?: { status?: string } }> = [];
    const socket = connectTourSocket((data) => events.push(JSON.parse(data)));
    socket.ready();
    await read(api(`threads/${TOUR_MAIN_THREAD}/prompt`), post({ prompt: 'hello' }));
    expect((await handleTourRequest(api(`threads/${TOUR_MAIN_THREAD}/prompt`), post({ prompt: 'again' }))).status).toBe(409);
    vi.advanceTimersByTime(8_000);
    expect(events.map((event) => event.type)).toEqual(expect.arrayContaining(['supervisor.connected', 'thread.turn.started', 'thread.item.completed', 'thread.output.delta']));
    await read(api(`threads/${TOUR_MAIN_THREAD}/interrupt`), { method: 'POST' });
    expect(events.find((event) => event.type === 'thread.turn.completed')?.payload?.status).toBe('interrupted');
    const detail = await read<{ thread: { status: string }; turns: Array<{ status: string }> }>(api(`threads/${TOUR_MAIN_THREAD}?limit=5`));
    expect(detail.thread.status).toBe('idle');
    expect(detail.turns.at(-1)!.status).toBe('interrupted');
    socket.close();
  });

  it('keeps shares made from its pages in memory', async () => {
    const share = await read<{ id: string; targetUsername: string }>('relay/shares', post({ targetIdentifier: 'alice', deviceId: 'pockymoe-tour', threadId: TOUR_MAIN_THREAD, threadAccess: 'read', workspaceAccess: 'none' }));
    expect(share.targetUsername).toBe('alice');
    const portal = await read<{ sharedByMe: Array<{ id: string }>; devices: Array<{ id: string }> }>('relay/portal');
    expect(portal.devices.map((device) => device.id)).toEqual(['pockymoe-tour']);
    expect(portal.sharedByMe.map((row) => row.id)).toEqual([share.id]);
    await read(`relay/shares/${share.id}`, { method: 'DELETE' });
    expect((await read<{ sharedByMe: unknown[] }>('relay/portal')).sharedByMe).toHaveLength(0);
  });
});

describe('tour terminal', () => {
  it('runs its few commands over the sample files', () => {
    const files = { 'README.md': '# demo\n', 'src/app.ts': 'x\n' };
    const shell = new TourShell('/home/you/projects/demo', () => files, () => []);
    const output: string[] = [];
    shell.listen((data) => output.push(data));
    shell.input('ls\r');
    shell.input('cat README.md\r');
    shell.input('cd src\rpwd\r');
    shell.input('rm -rf /\r');
    const text = output.join('');
    expect(text).toContain('README.md');
    expect(text).toContain('# demo');
    expect(text).toContain('/home/you/projects/demo/src');
    expect(text).toContain('rm: not in the sample terminal');
  });
});
