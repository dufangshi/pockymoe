import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import * as api from '../../lib/environmentApi';
import { requestFixture } from '../../test/environmentFixtures';
import { EnvironmentRequests } from './EnvironmentRequests';
vi.mock('../../lib/environmentApi', async original => ({ ...await original<typeof import('../../lib/environmentApi')>(), fetchEnvironmentRequests: vi.fn(), decideEnvironmentRequest: vi.fn() }));

beforeEach(() => { vi.clearAllMocks(); setLocale('en', false); vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([requestFixture()]); });
const review = async (name: RegExp = /Agent proposal/) => {
  fireEvent.click(await screen.findByRole('button', { name }));
  return within(await screen.findByRole('dialog', { name: 'Agent proposal' }));
};
it('reviews exact native/generic grants and approves the immutable request on its original device', async () => {
  const request=requestFixture(); vi.mocked(api.decideEnvironmentRequest).mockResolvedValue({ ...request, state: 'waitingIdle' });
  render(<EnvironmentRequests deviceId="device-a" threadId="thread-one" />);
  // Approval is only reachable from the full review, never from the list row.
  expect(screen.queryByRole('button', { name: 'Approve and migrate when idle' })).toBeNull();
  const dialog = await review();
  const approve=dialog.getByRole('button', { name: 'Approve and migrate when idle' });
  for (const path of ['/projects/original', '/outside/linked-worktree', '/private/native-codex', '/credentials/codex', '/private/service-file']) expect(dialog.getAllByTitle(path).length).toBeGreaterThan(0);
  expect(dialog.getByText('Native file login')).toBeVisible();
  expect(dialog.getByText(/long-term credential file is mapped into the environment/)).toBeVisible();
  expect(dialog.getByText('Host API broker')).toBeVisible();
  expect(dialog.getByText(/long-term key stays on Host/)).toBeVisible();
  expect(dialog.getByText(/Read-only files cannot persist refresh/)).toBeVisible();
  expect(dialog.getByText('Read and write')).toBeVisible();
  for (const host of ['chatgpt.com', 'auth.openai.com', 'github.com']) expect(dialog.getByText(host)).toBeVisible();
  expect(dialog.getByText('CODEX_HOME')).toBeVisible();
  expect(dialog.getByText('Keep the original owner constraint. Continue the unfinished work.')).toBeVisible();
  // The full immutable digest stays reviewable and copyable.
  expect(dialog.getByText('a'.repeat(64))).toBeInTheDocument();
  expect(dialog.getByRole('button', { name: 'Copy specification digest' })).toBeInTheDocument();
  expect(api.decideEnvironmentRequest).not.toHaveBeenCalled(); fireEvent.click(approve);
  await dialog.findByText('Approved, waiting for the current turn to exit');
  expect(api.decideEnvironmentRequest).toHaveBeenCalledExactlyOnceWith(request, 'approve', 'device-a');
  expect(dialog.queryByRole('button', { name: 'Approve and migrate when idle' })).toBeNull();
  expect(dialog.getByRole('list', { name: 'Migration progress' })).toBeVisible();
});
it('leaves shared recipients unable to approve or reject', async () => {
  render(<EnvironmentRequests deviceId="device-a" canManage={false} />);
  const dialog = await review();
  expect(dialog.getByRole('button', { name: 'Approve and migrate when idle' })).toBeDisabled();
  expect(dialog.getByRole('button', { name: 'Reject' })).toBeDisabled(); expect(api.decideEnvironmentRequest).not.toHaveBeenCalled();
  fireEvent.click(dialog.getByRole('button', { name: 'Why it is unavailable' }));
  expect(await screen.findByText('Only the thread owner can decide isolation requests.')).toBeVisible();
});
it('shows a safe decision failure and keeps the same reviewed proposal', async () => {
  vi.mocked(api.decideEnvironmentRequest).mockRejectedValue(new Error('private upstream response must not be rendered'));
  render(<EnvironmentRequests />); const dialog = await review();
  fireEvent.click(dialog.getByRole('button', { name: 'Approve and migrate when idle' }));
  await waitFor(() => expect(dialog.getByRole('button', { name: 'Approve and migrate when idle' })).toBeEnabled());
  expect(screen.queryByText(/private upstream response/)).toBeNull(); expect(await dialog.findByRole('button', { name: 'Why it failed' })).toBeVisible();
});
it.each(['failed', 'ready'] as const)('requires a separate confirmation before destroying %s computation and restoring Host', async state => {
  const request={...requestFixture(), state, approvedAt: '2026-11-07T00:00:01Z'};
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([request]);vi.mocked(api.decideEnvironmentRequest).mockResolvedValue({...request,state:'cancelled'});
  render(<EnvironmentRequests deviceId="device-a" />);
  const dialog = await review();
  if (state === 'failed') expect(dialog.getByRole('button', { name: 'Retry the approved specification' })).toBeEnabled();
  fireEvent.click(dialog.getByRole('button', {name:'Restore Host execution…'}));
  const confirm=within(await screen.findByRole('dialog', {name:'Abandon migration and restore Host?'}));
  // The confirmation opens on Cancel; nothing is destroyed until the explicit confirm.
  await waitFor(() => expect(confirm.getByRole('button', { name: 'Cancel' })).toHaveFocus());
  expect(api.decideEnvironmentRequest).not.toHaveBeenCalled();fireEvent.click(confirm.getByRole('button',{name:'Destroy computation and restore Host'}));
  await dialog.findByText('Cancelled');expect(api.decideEnvironmentRequest).toHaveBeenCalledExactlyOnceWith(request,'recoverHost','device-a');
});
it('lists pending requests first and translates the review', async () => {
  const older = { ...requestFixture(), id: 'older', state: 'ready' as const, createdAt: '2026-11-08T00:00:00Z', spec: { ...requestFixture().spec, name: 'Finished proposal' } };
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([older, requestFixture()]);
  render(<EnvironmentRequests deviceId="device-a" />);
  await screen.findByText('Finished proposal');
  const rows = within(screen.getByTestId('environment-request-list')).getAllByRole('button');
  expect(rows[0]).toHaveAccessibleName(/Agent proposal/);
  act(() => setLocale('zh-CN', false));
  const dialog = await review(/Agent proposal/);
  expect(dialog.getByRole('button', { name: '批准并在空闲后迁移' })).toBeVisible();
  expect(dialog.getByText('原生文件登录')).toBeVisible();
});
