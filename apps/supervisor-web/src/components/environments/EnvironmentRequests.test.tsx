import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import * as api from '../../lib/environmentApi';
import { requestFixture } from '../../test/environmentFixtures';
import { EnvironmentRequests } from './EnvironmentRequests';
vi.mock('../../lib/environmentApi', async original => ({ ...await original<typeof import('../../lib/environmentApi')>(), fetchEnvironmentRequests: vi.fn(), decideEnvironmentRequest: vi.fn() }));

beforeEach(() => { vi.clearAllMocks(); setLocale('en', false); vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([requestFixture()]); });
it('reviews exact native/generic grants and approves the immutable request on its original device', async () => {
  const request=requestFixture(); vi.mocked(api.decideEnvironmentRequest).mockResolvedValue({ ...request, state: 'waitingIdle' });
  render(<EnvironmentRequests deviceId="device-a" threadId="thread-one" />);
  const approve=await screen.findByRole('button', { name: 'Approve and migrate when idle' });
  for (const path of ['/projects/original', '/outside/linked-worktree', '/private/native-codex', '/credentials/codex', '/private/service-file']) expect(screen.getAllByTitle(path).length).toBeGreaterThan(0);
  expect(screen.getByText('Native subscription file login (direct long-term credential bind)')).toBeVisible();
  expect(screen.getByText('Host API broker (long-term key stays on Host)')).toBeVisible();
  expect(screen.getByText(/Read-only files cannot persist refresh/)).toBeVisible();
  expect(screen.getByText('Keep the original owner constraint. Continue the unfinished work.')).toBeVisible();
  expect(api.decideEnvironmentRequest).not.toHaveBeenCalled(); fireEvent.click(approve);
  await screen.findByText('Approved, waiting for the current turn to exit');
  expect(api.decideEnvironmentRequest).toHaveBeenCalledExactlyOnceWith(request, 'approve', 'device-a');
  expect(screen.queryByRole('button', { name: 'Approve and migrate when idle' })).toBeNull();
});
it('leaves shared recipients unable to approve or reject', async () => {
  render(<EnvironmentRequests deviceId="device-a" canManage={false} />);
  expect(await screen.findByRole('button', { name: 'Approve and migrate when idle' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Reject' })).toBeDisabled(); expect(api.decideEnvironmentRequest).not.toHaveBeenCalled();
});
it('shows a safe decision failure and keeps the same reviewed proposal', async () => {
  vi.mocked(api.decideEnvironmentRequest).mockRejectedValue(new Error('private upstream response must not be rendered'));
  render(<EnvironmentRequests />); fireEvent.click(await screen.findByRole('button', { name: 'Approve and migrate when idle' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Approve and migrate when idle' })).toBeEnabled());
  expect(screen.queryByText(/private upstream response/)).toBeNull(); expect(await screen.findByRole('button', { name: 'Why it failed' })).toBeVisible();
});
it.each(['failed', 'ready'] as const)('requires a separate confirmation before destroying %s computation and restoring Host', async state => {
  const request={...requestFixture(), state, approvedAt: '2026-11-07T00:00:01Z'};
  vi.mocked(api.fetchEnvironmentRequests).mockResolvedValue([request]);vi.mocked(api.decideEnvironmentRequest).mockResolvedValue({...request,state:'cancelled'});
  render(<EnvironmentRequests deviceId="device-a" />);
  if (state === 'ready') fireEvent.click(await screen.findByRole('button', {name: /Agent proposal/}));
  fireEvent.click(await screen.findByRole('button', {name:'Restore Host execution…'}));
  const dialog=within(await screen.findByRole('dialog', {name:'Abandon migration and restore Host?'}));
  expect(api.decideEnvironmentRequest).not.toHaveBeenCalled();fireEvent.click(dialog.getByRole('button',{name:'Destroy computation and restore Host'}));
  await screen.findByText('Cancelled');expect(api.decideEnvironmentRequest).toHaveBeenCalledExactlyOnceWith(request,'recoverHost','device-a');
});
