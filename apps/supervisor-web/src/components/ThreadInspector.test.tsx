import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import * as api from '../lib/environmentApi';
import { requestFixture } from '../test/environmentFixtures';
import { useThreadInspector } from './ThreadInspector';

vi.mock('../lib/environmentApi', async original => ({ ...await original<typeof import('../lib/environmentApi')>(), fetchEnvironmentRequests: vi.fn() }));
// The panels have their own tests; here they only report what they were given.
vi.mock('./git/ThreadGitPanel', () => ({ ThreadGitPanel: (props: { threadId: string; deviceId?: string | null }) => <p>git {props.deviceId}:{props.threadId}</p> }));
vi.mock('./environments/ThreadIsolationPanel', async original => ({ ...await original<typeof import('./environments/ThreadIsolationPanel')>(),
  ThreadIsolationPanel: (props: { thread: { id: string }; deviceId?: string | null; canManage: boolean }) => <p>isolation {props.deviceId}:{props.thread.id}:{String(props.canManage)}</p> }));

const thread = (id: string) => ({ id, title: `Thread ${id}`, status: 'idle' as const, provider: 'codex' as const, agentId: 'codex', providerSessionId: null, updatedAt: '2026-11-07', parentThreadId: null });
type HarnessProps = { id?: string; device?: string; filesOpen?: boolean; closeFiles?: () => void };
function Harness({ id = 'one', device = 'device-a', filesOpen = false, closeFiles = () => {} }: HarnessProps) {
  const { toolPanels, inspector } = useThreadInspector({ thread: thread(id), deviceId: device, canManage: true, gitAvailable: true, isolationAvailable: true, filesOpen, closeFiles });
  return <>
    <nav>{toolPanels.map(panel => <button key={panel.id} aria-label={panel.label} aria-pressed={panel.active} onClick={panel.onToggle}>{panel.icon}</button>)}</nav>
    {inspector && <aside aria-label={inspector.title}><small>{inspector.subtitle}</small>{inspector.content}<button onClick={inspector.onClose}>Close panel</button></aside>}
  </>;
}

beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); setLocale('en', false);
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([]);
});

it('names a pending approval on the rail entry and opens the isolation dock for that thread', async () => {
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([requestFixture()]);
  render(<Harness />);
  fireEvent.click(await screen.findByRole('button', { name: 'Isolated environments · approval needed' }));
  expect(screen.getByRole('complementary', { name: 'Isolated environments' })).toHaveTextContent('isolation device-a:one:true');
  expect(screen.getByText('Thread one')).toBeVisible();
  expect(api.fetchEnvironmentRequests).toHaveBeenCalledWith('device-a', 'one', expect.any(AbortSignal));
});

it('toggles one view at a time, remembers only which, and never carries content across threads', async () => {
  const view = render(<Harness />);
  fireEvent.click(screen.getByRole('button', { name: 'Git' }));
  expect(screen.getByRole('button', { name: 'Git' })).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByText('git device-a:one')).toBeVisible();
  expect(localStorage.getItem('pockymoe.workbench.inspector')).toBe('git');
  expect(JSON.stringify(localStorage)).not.toContain('device-a');
  view.rerender(<Harness id="two" device="device-b" />);
  expect(screen.getByText('git device-b:two')).toBeVisible();
  expect(screen.queryByText('git device-a:one')).toBeNull();
  expect(screen.getByText('Thread two')).toBeVisible();
  await waitFor(() => expect(api.fetchEnvironmentRequests).toHaveBeenLastCalledWith('device-b', 'two', expect.any(AbortSignal)));
  fireEvent.click(screen.getByRole('button', { name: 'Isolated environments' }));
  expect(screen.queryByText(/^git /)).toBeNull();
  expect(screen.getByText('isolation device-b:two:true')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Close panel' }));
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(localStorage.getItem('pockymoe.workbench.inspector')).toBeNull();
});

it('shares the dock with Files: opening one closes the other', () => {
  const closeFiles = vi.fn();
  const view = render(<Harness filesOpen closeFiles={closeFiles} />);
  fireEvent.click(screen.getByRole('button', { name: 'Git' }));
  expect(closeFiles).toHaveBeenCalledOnce();
  view.rerender(<Harness closeFiles={closeFiles} />);
  expect(screen.getByText('git device-a:one')).toBeVisible();
  view.rerender(<Harness filesOpen closeFiles={closeFiles} />);
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(screen.getByRole('button', { name: 'Git' })).toHaveAttribute('aria-pressed', 'false');
});
