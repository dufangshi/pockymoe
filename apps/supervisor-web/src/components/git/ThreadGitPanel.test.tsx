import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ApiError } from '../../lib/api';
import { ThreadGitPanel } from './ThreadGitPanel';
import * as git from '../../lib/gitApi';

vi.mock('../../lib/gitApi', () => ({ fetchCheckpoints: vi.fn().mockResolvedValue({ turns: [] }), fetchGitBranches: vi.fn(), gitWrite: vi.fn(), fetchThreadWorkingContext: vi.fn(), fetchThreadGitStatus: vi.fn(), fetchThreadGitDiff: vi.fn(), fetchThreadGitCommits: vi.fn() }));
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
  it('loads for its thread and keeps the working-copy facts on one line, with details on request', async () => {
    render(<ThreadGitPanel threadId="thread" deviceId="device" />);
    await screen.findByRole('button', { name: 'View diff for src/app.rs' });
    expect(git.fetchThreadGitStatus).toHaveBeenCalledWith('thread', 'a', 'device', expect.any(AbortSignal));
    // The branch shows once (in the switch), HEAD short with a copy of the full ID.
    expect(screen.getAllByText('feature/a')).toHaveLength(1);
    expect(screen.getByText('abc')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy HEAD commit ID' })).toBeVisible();
    // Role, path and the view-only note wait behind Details.
    expect(screen.queryByText('Primary working copy')).toBeNull();
    expect(screen.queryByText('Selecting a working copy here only changes this view.')).toBeNull();
    const details = screen.getByRole('button', { name: 'Working copy details' });
    fireEvent.click(details);
    expect(details).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('Primary working copy')).toBeVisible();
    expect(screen.getByText('Created by Pockymoe')).toBeVisible();
    expect(screen.getByText('Selecting a working copy here only changes this view.')).toBeVisible();
    expect(screen.getAllByRole('button', { name: 'Copy absolute path' })).toHaveLength(2);
    // The operation elsewhere is one line naming the other copy.
    expect(screen.getByRole('note')).toHaveTextContent('Latest operation in feature/b');
    fireEvent.click(screen.getByRole('button', { name: 'View diff for src/app.rs' }));
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'worktree', path: 'src/app.rs' }, 'device', expect.any(AbortSignal)));
    // A docked inspector, not a window: no dialog or focus trap.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('discards a late diff response after selecting another worktree', async () => {
    const first = pending<git.GitDiff>();
    vi.mocked(git.fetchThreadGitDiff).mockImplementationOnce(() => first.promise);
    render(<ThreadGitPanel threadId="thread" />);
    const selector = await screen.findByRole('combobox', { name: 'Working copy' });
    fireEvent.click(selector);
    fireEvent.click(await screen.findByRole('option', { name: 'feature/b · b' }));
    await screen.findByText('diff b');
    await act(async () => first.resolve({ workingCopyId: 'a', area: 'worktree', patch: 'stale patch from a', truncated: false }));
    expect(screen.queryByText('stale patch from a')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Working copy details' }));
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
    expect(screen.getByRole('button', { name: '提交' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: '下一页' })).toBeVisible();
    expect(screen.getByRole('note')).toHaveTextContent('最近一次操作在 feature/b');
  });

  it('opens what a commit changed from the commit list, and goes back to the list', async () => {
    render(<ThreadGitPanel threadId="thread" />); await screen.findByText('diff a');
    fireEvent.click(screen.getByRole('button', { name: 'Commits' }));
    fireEvent.click(await screen.findByRole('button', { name: 'View the changes of commit “Fix worktree paths”' }));
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'commit', base: 'abc123' }, undefined, expect.any(AbortSignal)));
    // The commit's short ID, its subject and a copy of the full ID head the view.
    expect(await screen.findByText('abc123')).toBeVisible();
    expect(screen.getByText('Fix worktree paths')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy commit ID' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Commits' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByRole('button', { name: 'Back to commits' }));
    expect(await screen.findByRole('button', { name: 'View the changes of commit “Fix worktree paths”' })).toBeVisible();
  });

  it('has no chooser, source tag or explanation for a single working copy, and no zero counts', async () => {
    vi.mocked(git.fetchThreadWorkingContext).mockResolvedValue({ ...context, workingCopies: [{ ...context.workingCopies[0]!, source: 'configured' }], observations: [] });
    vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, truncated: false,
      patch: 'diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1 +1,2 @@\n line\n+new line\n' }));
    render(<ThreadGitPanel threadId="thread" />);
    await screen.findByText('feature/a');
    expect(screen.queryByRole('combobox', { name: 'Working copy' })).toBeNull();
    expect(screen.queryByRole('note')).toBeNull();
    expect(screen.getByRole('button', { name: 'Refresh Git status' })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Working copy details' }));
    expect(screen.queryByText('Selecting a working copy here only changes this view.')).toBeNull();
    expect(screen.queryByText('Primary working copy')).toBeNull();
    expect(screen.getByRole('button', { name: 'Copy absolute path' })).toBeVisible();
    const files = within(screen.getByRole('complementary', { name: 'Changed files' }));
    await waitFor(() => expect(files.getByRole('button', { name: 'View diff for src/app.rs' })).toHaveTextContent('+1'));
    expect(files.getByRole('button', { name: 'View diff for src/app.rs' })).not.toHaveTextContent('−0');
  });

  it('lists files per scope with counts and colors the selected file diff', async () => {
    vi.mocked(git.fetchThreadGitStatus).mockResolvedValue({ workingCopyId: 'a', branch: 'feature/a', head: 'abc', truncated: false, files: [
      { path: 'src/app.rs', indexStatus: 'M', worktreeStatus: ' ' }, { path: 'docs/new.md', indexStatus: '?', worktreeStatus: '?' },
    ] });
    vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, truncated: false,
      patch: 'diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1 +1 @@\n-old line\n+new line\n' }));
    render(<ThreadGitPanel threadId="thread" />);
    const scope = within(await screen.findByRole('group', { name: 'Git view' }));
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
    expect(screen.getByRole('combobox', { name: 'Working copy' })).toHaveTextContent('feature/b');
    expect(screen.queryByRole('button', { name: 'View that copy' })).toBeNull();
    expect(screen.getByRole('note')).toHaveTextContent('Latest operation was here');
    expect(git.fetchThreadGitStatus).toHaveBeenLastCalledWith('thread', 'b', undefined, expect.any(AbortSignal));
  });

  it('shows a binary file and an untracked file each under its own name', async () => {
    vi.mocked(git.fetchThreadGitStatus).mockResolvedValue({ workingCopyId: 'a', branch: 'feature/a', head: 'abc', truncated: false, files: [
      { path: 'docs/diagram.png', indexStatus: '?', worktreeStatus: '?' }, { path: 'sup2.pid', indexStatus: '?', worktreeStatus: '?' },
    ] });
    vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, truncated: false,
      patch: input.path === 'sup2.pid' ? '' : '--- /dev/null\n+++ b/docs/diagram.png\nBinary files /dev/null and b/docs/diagram.png differ\n--- /dev/null\n+++ b/sup2.pid\n@@ -0,0 +1 @@\n+1087690\n' }));
    render(<ThreadGitPanel threadId="thread" />);
    const files = within(await screen.findByRole('complementary', { name: 'Changed files' }));
    await waitFor(() => expect(files.getByRole('button', { name: 'View diff for docs/diagram.png' })).toHaveTextContent('Binary'));
    expect(files.getByRole('button', { name: 'View diff for sup2.pid' })).toHaveTextContent('+1');
    expect(files.getByText('2 files')).toBeVisible();
    // The scope's combined diff is already under the list; nothing is opened.
    expect(screen.getByTestId('thread-git')).not.toHaveAttribute('data-opened');
    const diff = within(screen.getByRole('region', { name: 'All files' }));
    expect(diff.getByRole('button', { name: 'View diff for docs/diagram.png' })).toHaveTextContent('Binary');
    expect(diff.getByText('New binary file; contents are not shown as text')).toBeVisible();
    expect(diff.queryByText(/Binary files \/dev\/null/)).toBeNull();
    const pid = diff.getByRole('button', { name: 'View diff for sup2.pid' }).closest('section')!;
    expect(within(pid as HTMLElement).getByText('+1087690')).toBeVisible();
    // A file heading in the combined diff opens that file on its own; an empty
    // untracked file says so instead of "no diff".
    fireEvent.click(diff.getByRole('button', { name: 'View diff for sup2.pid' }));
    expect(screen.getByTestId('thread-git')).toHaveAttribute('data-opened', 'true');
    expect(await screen.findByText('Empty file')).toBeVisible();
    // All diffs (shown beside the list in a wide dock) goes back to every file without opening one.
    fireEvent.click(screen.getByRole('button', { name: 'All diffs' }));
    expect(screen.getByTestId('thread-git')).not.toHaveAttribute('data-opened');
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'worktree' }, undefined, expect.any(AbortSignal)));
  });

  it('steps through files from the diff and returns to the list', async () => {
    vi.mocked(git.fetchThreadGitStatus).mockResolvedValue({ workingCopyId: 'a', branch: 'feature/a', head: 'abc', truncated: false, files: [
      { path: 'a.txt', indexStatus: ' ', worktreeStatus: 'M' }, { path: 'b.txt', indexStatus: ' ', worktreeStatus: 'M' },
    ] });
    render(<ThreadGitPanel threadId="thread" />);
    fireEvent.click(await screen.findByRole('button', { name: 'View diff for a.txt' }));
    expect(screen.getByTestId('thread-git')).toHaveAttribute('data-opened', 'true');
    expect(screen.getByRole('button', { name: 'Previous file' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Next file' }));
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'worktree', path: 'b.txt' }, undefined, expect.any(AbortSignal)));
    expect(screen.getByRole('button', { name: 'Next file' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Back to files' }));
    expect(screen.getByTestId('thread-git')).not.toHaveAttribute('data-opened');
    // Back lands on the row of the file that was open.
    expect(screen.getByRole('button', { name: 'View diff for b.txt' })).toHaveFocus();
    await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'worktree' }, undefined, expect.any(AbortSignal)));
  });

  it('says a clean working tree is clean', async () => {
    vi.mocked(git.fetchThreadGitStatus).mockResolvedValue({ workingCopyId: 'a', branch: 'feature/a', head: 'abc', truncated: false, files: [] });
    vi.mocked(git.fetchThreadGitDiff).mockImplementation(async (_id, copy, input) => ({ workingCopyId: copy, area: input.area, truncated: false, patch: '' }));
    render(<ThreadGitPanel threadId="thread" />);
    expect(await screen.findByText('Working tree clean')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'All diffs' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^Staged/ }));
    expect(await screen.findByText('Nothing staged')).toBeVisible();
  });
});

it('opens turn diffs and stages only the selected file through owner controls', async () => {
  vi.mocked(git.fetchCheckpoints).mockResolvedValue({ turns: [{ turnId: 'turn-1', turnIndex: 1, startedAt: '2026-11-01T12:00:00Z', status: 'completed', workingCopies: [{ workingCopyId: 'a', available: true, files: 1, additions: 2, deletions: 1 }] }] });
  vi.mocked(git.gitWrite).mockResolvedValue({ status: { workingCopyId: 'a', branch: 'main', head: 'abc', files: [], truncated: false } });
  render(<ThreadGitPanel threadId="thread" canWrite />);
  await screen.findByRole('button', { name: 'View diff for src/app.rs' });
  fireEvent.click(screen.getByRole('button', { name: 'Stage' }));
  await waitFor(() => expect(git.gitWrite).toHaveBeenCalledWith('thread', 'git/stage', { workingCopyId: 'a', paths: ['src/app.rs'], confirmRunning: false }, undefined));
  fireEvent.click(screen.getByRole('button', { name: 'Turn' }));
  await waitFor(() => expect(git.fetchThreadGitDiff).toHaveBeenLastCalledWith('thread', 'a', { area: 'turn', turnId: 'turn-1' }, undefined, expect.any(AbortSignal)));
  expect(screen.getByRole('button', { name: 'Restore before' })).toBeEnabled();
});
