// @vitest-environment node
import { beforeEach, expect, it, vi } from 'vitest';
const pin = vi.hoisted(() => vi.fn());
vi.mock('./sharedDeviceTrust', () => ({ pinInviteOwner: pin }));
import {
  acceptSharedInvite,
  parseSharedInvite,
  sharedInviteUrl,
} from './sharedInvite';
const ownerAccountKeyId = 'g'.repeat(43);
beforeEach(() => {
  pin.mockReset();
});
it('round trips thread and device invites with the pin exclusively in the fragment', async () => {
  for (const invite of [
    { deviceId: 'device', ownerAccountKeyId },
    { deviceId: 'device /', ownerAccountKeyId, threadId: 'thread /?' },
  ]) {
    const url = new URL(sharedInviteUrl('https://pockymoe.test', invite));
    expect(url.pathname).toBe('/shared');
    expect(url.search).toBe('');
    expect(parseSharedInvite(url.hash)).toEqual(invite);
    const path = await acceptSharedInvite(invite);
    expect(pin).toHaveBeenLastCalledWith(invite.deviceId, ownerAccountKeyId);
    expect(path).toBe(
      `/devices/${encodeURIComponent(invite.deviceId)}/${'threadId' in invite ? 'threads/' + encodeURIComponent(invite.threadId!) : 'workspaces'}`,
    );
  }
});
it.each([
  '',
  '#v=1&device=d&owner=' + ownerAccountKeyId,
  '#v=2&owner=' + ownerAccountKeyId,
  '#v=2&device=d&owner=wrong',
  '#v=2&device=d&owner=' + ownerAccountKeyId + '&owner=' + ownerAccountKeyId,
  '#v=2&device=d&owner=' + ownerAccountKeyId + '&thread=',
])('rejects incomplete, duplicate or unsupported invite %s', (fragment) => {
  expect(() => parseSharedInvite(fragment)).toThrow(/invalid/);
});
it('fails closed when an invite conflicts with the stored ownerAccountKeyId pin', async () => {
  pin.mockRejectedValue(new Error('Device ownerAccountKeyId mismatch'));
  await expect(
    acceptSharedInvite({ deviceId: 'device', ownerAccountKeyId }),
  ).rejects.toThrow('Device ownerAccountKeyId mismatch');
});
