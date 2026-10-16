import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { SharingApprovalDialog } from './SharingApprovalDialog';
import { requestGrantReview } from '../lib/sharingApproval';

const description = 'Device: Laptop (device-id)\nScope: thread /thread-123\nAccess: read, none; create threads: false\nExpiry: no expiry\nRecipient: Alice\nSafety code: AAAA BBBB CCCC';
it('shows exact grant fields and requires acknowledgement plus an explicit signing click for unverified recipients', async () => {
  render(<SharingApprovalDialog />);
  let result!: Promise<boolean>;
  act(() => { result = requestGrantReview({ description, recipientVerified: false }); });
  expect(screen.getByRole('dialog')).toHaveTextContent('Laptop (device-id)');
  for (const field of ['thread-123', 'read, none', 'no expiry', 'Alice', 'AAAA BBBB CCCC'])
    expect(screen.getByRole('dialog')).toHaveTextContent(field);
  const sign = screen.getByRole('button', { name: 'Sign and share' });
  expect(sign).toBeDisabled();
  let completed = false;
  void result.then(() => { completed = true; });
  await act(async () => { await Promise.resolve(); });
  expect(completed).toBe(false);
  fireEvent.click(screen.getByRole('checkbox', { name: /Recipient not verified/ }));
  expect(sign).toBeEnabled();
  expect(completed).toBe(false);
  fireEvent.click(sign);
  expect(await result).toBe(true);
});
it('still requires an explicit click for verified recipients and cancels without authorizing', async () => {
  render(<SharingApprovalDialog />);
  let result!: Promise<boolean>;
  act(() => { result = requestGrantReview({ description, recipientVerified: true }); });
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Sign and share' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(await result).toBe(false);
});
it('fails closed when the confirmation UI is unavailable', async () => {
  expect(await requestGrantReview({ description, recipientVerified: true })).toBe(false);
});
it('resets acknowledgement when the next queued credential set is reviewed', async () => {
  render(<SharingApprovalDialog />);
  let first!: Promise<boolean>, second!: Promise<boolean>;
  act(() => {
    first = requestGrantReview({ description, recipientVerified: false });
    second = requestGrantReview({ description: 'Recipient: Bob\nSafety code: DDDD EEEE FFFF', recipientVerified: false });
  });
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Sign and share' }));
  expect(await first).toBe(true);
  expect(screen.getByRole('dialog')).toHaveTextContent('Bob');
  expect(screen.getByRole('button', { name: 'Sign and share' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(await second).toBe(false);
});
it('describes the grant in words, lists changed fields and folds the signed content', async () => {
  const summary = {
    device: { name: 'zzq docker', id: 'device-1' },
    recipient: { name: 'Alice', userId: 'user-1' },
    safetyCode: 'AAAA BBBB CCCC',
    scope: 'device' as const, workspaceScope: 'all' as const, workspaceCount: 0,
    threadAccess: 'control' as const, workspaceAccess: 'read' as const,
    canCreateThreads: false, expiresAt: null,
  };
  const before = { ...summary, threadAccess: 'read' as const };
  render(<SharingApprovalDialog />);
  let result!: Promise<boolean>;
  act(() => {
    result = requestGrantReview({
      description: '{"workspaceScope":"all","grantee":{"userId":"user-1"}}',
      recipientVerified: true,
      summary: { ...summary, changes: [{ field: 'threadAccess', before, after: summary }] },
    });
  });
  const dialog = screen.getByRole('dialog');
  expect(dialog).toHaveTextContent('Allow Alice to use zzq docker');
  for (const text of ['Verified', 'The whole device, all workspaces', 'Read, send messages and control', 'Read files', 'Not allowed', 'No expiry', 'AAAA BBBB CCCC'])
    expect(dialog).toHaveTextContent(text);
  expect(dialog).toHaveTextContent('Conversations' + 'Read only → Read, send messages and control');
  const details = dialog.querySelector('details')!;
  expect(details.open).toBe(false);
  expect(details).toHaveTextContent('"workspaceScope":"all"');
  fireEvent.click(screen.getByRole('button', { name: 'Sign and share' }));
  expect(await result).toBe(true);
});
