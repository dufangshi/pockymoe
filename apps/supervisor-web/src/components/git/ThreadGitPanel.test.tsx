import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ApiError } from '../../lib/api';
import { ThreadGitPanel } from './ThreadGitPanel';
import * as git from '../../lib/gitApi';

vi.mock('../../lib/gitApi', () => ({ fetchThreadWorkingContext: vi.fn(), fetchThreadGitStatus: vi.fn(), fetchThreadGitDiff: vi.fn(), fetchThreadGitCommits: vi.fn() }));
const context: git.ThreadWorkingContext = {
  configuredCwd: '/repo', primaryWorkingCopyId: 'a',
  workingCopies: [
    { id: 'a', repositoryId: 'repo', path: '/repo.worktrees/a', branch: 'feature/a', head: 'aaaaaaaa', managed: true, source: 'created', lastObservedAt: null },
    { id: 'b', repositoryId: 'repo', path: '/repo.worktrees/b', branch: 'feature/b', head: 'bbbbbbbb', managed: false, source: 'observed', lastObservedAt: null },
  ], observations: [{ workingCopyId: 'b', cwd: '/repo.worktrees/b/src', source: 'commandCwd', toolCallId: 'tool', agentId: null, observedAt: '2026-11-01T12:00:00Z' }],
};
const pending = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; };

beforeEach(() => {
  vi.clearAllMocks();
  setLocale('en');
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
  vi.mocked(git.fetchThreadWorkingContext).mockResolvedValue(context);
  vi.mocked(git.fetchThreadGitStatus).mockImplementation(async (_id, copy) => ({ workingCopyId: copy, branch: `feature/${copy}`, head: 'abc', files: [{ path: 'src/app.rs', indexStatus: ' ', worktreeStatus: 'M' }], truncated: false }));
  vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, patch: `diff ${copy}`, truncated: false }));
  vi.mocked(git.fetchThreadGitCommits).mockResolvedValue({ commits: [{ oid: 'abc123', subject: 'Fix worktree paths', author: 'Dev', authoredAt: '2026-11-01T12:00:00Z' }], hasMore: true });
});

describe('thread Git inspection', () => {
  it('loads for its thread and separates the primary checkout from observed operations', async () => {
    render(<ThreadGitPanel threadId="thread" deviceId="device" />);
    await screen.findByText('diff a');
    expect(screen.getByText('Primary working copy')).toBeVisible();
    expect(screen.getByText('Recent operation outside the primary working copy')).toBeVisible();
    expect(screen.getByText('Selecting a working copy here only changes this view.')).toBeVisible();
    expect(git.fetchThreadGitStatus).toHaveBeenCalledWith('thread', 'a', 'device', expect.any(AbortSignal));
    fireEvent.click(screen.getByRole('button', { name: 'View diff for src/app.rs' }));
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'worktree', path: 'src/app.rs' }, 'device', expect.any(AbortSignal)));
    // A docked inspector, not a window: no dialog or focus trap.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('discards a late diff response after selecting another worktree', async () => {
    const first = pending<git.GitDiff>();
    vi.mocked(git.fetchThreadGitDiff).mockImplementationOnce(() => first.promise);
    render(<ThreadGitPanel threadId="thread" />);
    await screen.findByText('Primary working copy');
    const selector = screen.getByRole('combobox', { name: 'Working copy' });
    fireEvent.click(selector);
    fireEvent.click(await screen.findByRole('option', { name: 'feature/b · /repo.worktrees/b' }));
    await screen.findByText('diff b');
    await act(async () => first.resolve({ workingCopyId: 'a', area: 'worktree', patch: 'stale patch from a', truncated: false }));
    expect(screen.queryByText('stale patch from a')).not.toBeInTheDocument();
    expect(screen.getByText('Viewing this working copy')).toBeVisible();
  });

  it.each([403, 404])('shows an unavailable explanation when the device returns %s', async code => {
    vi.mocked(git.fetchThreadWorkingContext).mockRejectedValue(new ApiError(code, { code: 'service_unavailable', message: 'Device refused' }));
    render(<ThreadGitPanel threadId="thread" />);
    await screen.findByRole('button', { name: 'Why it is unavailable' });
    expect(git.fetchThreadGitDiff).not.toHaveBeenCalled();
    expect(screen.queryByText('Loading Git data…')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
    expect(await screen.findByText(code === 403 ? 'Git inspection needs owner access or write access to the whole device.' : 'Git inspection is unavailable on this Supervisor.')).toBeVisible();
  });

  it('defers hidden-tab invalidations, then refreshes once visible', async () => {
    const view = render(<ThreadGitPanel threadId="thread" refreshKey="one" />);
    await screen.findByText('diff a');
    const count = vi.mocked(git.fetchThreadWorkingContext).mock.calls.length;
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
    fireEvent(document, new Event('visibilitychange'));
    view.rerender(<ThreadGitPanel threadId="thread" refreshKey="two" />);
    await act(async () => {});
    expect(git.fetchThreadWorkingContext).toHaveBeenCalledTimes(count);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    fireEvent(document, new Event('visibilitychange'));
    await waitFor(() => expect(git.fetchThreadWorkingContext).toHaveBeenCalledTimes(count + 1));
  });

  it('pages commits on demand and translates the view without changing selection', async () => {
    render(<ThreadGitPanel threadId="thread" />); await screen.findByText('diff a');
    expect(git.fetchThreadGitCommits).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Commits' }));
    await screen.findByText('Fix worktree paths');
    fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
    await waitFor(() => expect(git.fetchThreadGitCommits).toHaveBeenLastCalledWith('thread', 'a', 30, undefined, expect.any(AbortSignal)));
    act(() => setLocale('zh-CN'));
    expect(screen.getByText('主工作副本')).toBeVisible();
    expect(screen.getByRole('button', { name: '下一页' })).toBeVisible();
  });

  it('lists files per scope with counts and colors the selected file diff', async () => {
    vi.mocked(git.fetchThreadGitStatus).mockResolvedValue({ workingCopyId: 'a', branch: 'feature/a', head: 'abc', truncated: false, files: [
      { path: 'src/app.rs', indexStatus: 'M', worktreeStatus: ' ' }, { path: 'docs/new.md', indexStatus: '?', worktreeStatus: '?' },
    ] });
    vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, truncated: false,
      patch: 'diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1 +1 @@\n-old line\n+new line\n' }));
    render(<ThreadGitPanel threadId="thread" />);
    const scope = within(await screen.findByRole('group', { name: 'Diff scope' }));
    await waitFor(() => expect(scope.getByRole('button', { name: 'Unstaged 1' })).toHaveAttribute('aria-pressed', 'true'));
    const files = within(screen.getByRole('complementary', { name: 'Changed files' }));
    expect(files.getByRole('button', { name: 'View diff for docs/new.md' })).toBeVisible();
    expect(files.queryByRole('button', { name: 'View diff for src/app.rs' })).toBeNull();
    fireEvent.click(scope.getByRole('button', { name: 'Staged 1' }));
    fireEvent.click(await files.findByRole('button', { name: 'View diff for src/app.rs' }));
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'index', path: 'src/app.rs' }, undefined, expect.any(AbortSignal)));
    expect((await screen.findByText('+new line')).closest('[data-kind]')).toHaveAttribute('data-kind', 'add');
    expect(screen.getByText('-old line').closest('[data-kind]')).toHaveAttribute('data-kind', 'del');
  });

  it('jumps to the working copy of a recent operation without changing the thread', async () => {
    render(<ThreadGitPanel threadId="thread" />);
    await screen.findByText('diff a');
    fireEvent.click(screen.getByRole('button', { name: 'View that copy' }));
    await screen.findByText('diff b');
    expect(screen.getByRole('combobox', { name: 'Working copy' })).toHaveTextContent('feature/b · /repo.worktrees/b');
    expect(screen.queryByRole('button', { name: 'View that copy' })).toBeNull();
    expect(git.fetchThreadGitStatus).toHaveBeenLastCalledWith('thread', 'b', undefined, expect.any(AbortSignal));
  });
});
