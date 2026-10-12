// @vitest-environment node
import { beforeEach, expect, it, vi } from 'vitest';
const pin = vi.hoisted(() => vi.fn());
vi.mock('./ownerDeviceTrust', () => ({ pinInviteIdentity: pin }));
import {
  acceptSharedInvite,
  parseSharedInvite,
  sharedInviteUrl,
} from './sharedInvite';
const identity = 'g'.repeat(43);
beforeEach(() => {
  pin.mockReset();
});
it('round trips thread and device invites with the pin exclusively in the fragment', async () => {
  for (const invite of [
    { deviceId: 'device', identity },
    { deviceId: 'device /', identity, threadId: 'thread /?' },
  ]) {
    const url = new URL(sharedInviteUrl('https://pockymoe.test', invite));
    expect(url.pathname).toBe('/shared');
    expect(url.search).toBe('');
    expect(parseSharedInvite(url.hash)).toEqual(invite);
    const path = await acceptSharedInvite(invite);
    expect(pin).toHaveBeenLastCalledWith(invite.deviceId, identity);
    expect(path).toBe(
      `/devices/${encodeURIComponent(invite.deviceId)}/${'threadId' in invite ? 'threads/' + encodeURIComponent(invite.threadId!) : 'workspaces'}`,
    );
  }
});
it.each([
  '',
  '#v=2&device=d&identity=' + identity,
  '#v=1&identity=' + identity,
  '#v=1&device=d&identity=wrong',
  '#v=1&device=d&identity=' + identity + '&identity=' + identity,
  '#v=1&device=d&identity=' + identity + '&thread=',
])('rejects incomplete, duplicate or unsupported invite %s', (fragment) => {
  expect(() => parseSharedInvite(fragment)).toThrow(/invalid/);
});
it('fails closed when an invite conflicts with the stored identity pin', async () => {
  pin.mockRejectedValue(new Error('Device identity mismatch'));
  await expect(
    acceptSharedInvite({ deviceId: 'device', identity }),
  ).rejects.toThrow('Device identity mismatch');
});
