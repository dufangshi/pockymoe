// @vitest-environment node
import { beforeEach, expect, it, vi } from 'vitest';
import vectors from './fixtures/accountKeyVectors.json';
import type { RelaySessionShareDto } from '@pockymoe/shared';
const state = vi.hoisted(() => ({ request: vi.fn(), approve: vi.fn(), bound: vi.fn(), revocations: vi.fn(), account: vi.fn(), store: new Map<string, unknown>() }));
vi.mock('./api', () => ({ request: state.request }));
vi.mock('./sharingApproval', () => ({ requestGrantReview: state.approve }));
vi.mock('./accountVault', () => ({ currentAccount: state.account, boundDevices: state.bound, accountRevocations: state.revocations,
  safetyCode: async () => 'VDNZ DB8Z 3HKC', verifyDeviceCertificate: async () => true }));
vi.mock('./sharingTrustStore', () => ({ sharingTrustStore: async (key: string, ...write: unknown[]) => {
  if (write.length) { if (write[0] === null) state.store.delete(key); else state.store.set(key, structuredClone(write[0])); }
  return state.store.get(key);
}, sharingTrustEntries: async (prefix: string) => [...state.store.entries()].filter(([key]) => key.startsWith(prefix)) }));
vi.mock('./sharedDeviceTrust', async original => ({ ...await original<object>(), isProtectedSharingDevice: async () => true }));
const sharing = await import('./signedSharing');
const trust = await import('./sharedDeviceTrust');
const account = { accountKeyId: vectors.account.accountKeyId, accountPublicKey: vectors.account.accountPublicKey };
const row = { id: 'row', deviceId: 'device', deviceName: 'Device', targetUserId: 'recipient', targetUsername: 'Recipient', threadId: 'thread', workspaceId: null,
  threadAccess: 'read', workspaceAccess: 'none', expiresAt: null } as RelaySessionShareDto;
beforeEach(() => {
  state.store.clear(); state.bound.mockReset().mockResolvedValue([]); state.revocations.mockReset().mockResolvedValue([]);
  state.account.mockReset().mockResolvedValue(vectors.account); state.approve.mockReset().mockResolvedValue(true);
  state.request.mockReset().mockImplementation(async (path: string) => {
    if (path === 'api/transport/grants') return { grants: [] };
    if (path === 'relay/portal') return { sharedByMe: [row] };
    if (path.endsWith('/recipient-account')) return account;
    return {};
  });
});
it('revokes on vault-bound devices with no local certificate and on locally certified devices, once per device', async () => {
  state.bound.mockResolvedValue(['bound-only', 'certified']);
  state.store.set(`device-certificate:${account.accountKeyId}:certified`, { ...vectors.deviceCertificate.certificate, deviceId: 'certified' });
  const grant = sharing.grantForRow('share', row, account, 'acl');
  state.request.mockResolvedValue({ grants: [grant] });
  const mutate = vi.fn(); await sharing.sharingMutation('share', 'revoke', row.id, mutate);
  for (const deviceId of ['bound-only', 'certified'])
    expect(state.request).toHaveBeenCalledWith('api/transport/grants/acl', { method: 'DELETE' }, { deviceId });
  expect(state.request.mock.calls.filter(c => c[0] === 'api/transport/grants')).toHaveLength(2);
  expect(mutate).toHaveBeenCalledOnce();
});
it('fails closed before relay removal or device deletions if an uncertified bound device cannot be reached', async () => {
  state.bound.mockResolvedValue(['offline']); state.request.mockRejectedValue({ statusCode: 503 });
  const mutate = vi.fn();
  await expect(sharing.sharingMutation('share', 'revoke', row.id, mutate)).rejects.toThrow(/must be online/);
  expect(mutate).not.toHaveBeenCalled();
  expect(state.request.mock.calls.some(c => c[1]?.method === 'DELETE')).toBe(false);
});
it('skips a removed certified device only when a verified current-account device revocation exists', async () => {
  state.bound.mockResolvedValue(['live', 'removed', 'browser-revoked']);
  state.store.set(`device-certificate:${account.accountKeyId}:removed`, { ...vectors.deviceCertificate.certificate, deviceId: 'removed' });
  state.revocations.mockResolvedValue([
    { ...vectors.revocations.device, subjectId: 'removed' },
    { ...vectors.revocations.browser, subjectId: 'browser-revoked' },
    { ...vectors.revocations.device, accountKeyId: vectors.otherAccount.accountKeyId, subjectId: 'live' },
  ]);
  const mutate = vi.fn(); await sharing.sharingMutation('share', 'revoke', row.id, mutate);
  expect(state.request.mock.calls.map(c => c[2]?.deviceId)).toEqual(['live', 'browser-revoked']);
  expect(mutate).toHaveBeenCalledOnce();
});
it('never lets an incomplete edit inherit relay-selected scope, target, expiry or permissions', async () => {
  const mutate = vi.fn(async () => ({ ...row, threadAccess: 'control' }));
  await expect(sharing.sharingMutation('share', 'edit', row.id, mutate, { threadAccess: 'control' })).rejects.toThrow(/dialog/i);
  expect(mutate).not.toHaveBeenCalled(); expect(state.approve).not.toHaveBeenCalled();
});
it('shows the signed old-to-new permission difference for a complete dialog edit', async () => {
  const old = sharing.grantForRow('share', row, account, 'acl');
  const server = state.request.getMockImplementation()!;
  state.request.mockImplementation(async (path, ...args) => path === 'api/transport/grants' ? { grants: [old] } : server(path, ...args));
  await sharing.sharingMutation('share', 'edit', row.id, async () => ({ ...row, threadAccess: 'control' }), {
    ...row, threadAccess: 'control', scope: 'thread', workspaceScope: 'all', workspaceIds: [], canCreateThreads: false,
  });
  const review = state.approve.mock.calls[0]![0].description;
  expect(review).toMatch(/Before/); expect(review).toMatch(/After/);
  expect(review).toContain('"threadAccess":"read"'); expect(review).toContain('"threadAccess":"control"');
  expect(review).toContain('Changed fields (before → after): {"threadAccess":{"before":"read","after":"control"}}');
});
it('binds a verified recipient to both its relay user id and its account key', async () => {
  await sharing.confirmRecipient(account, 'recipient');
  expect(await sharing.recipientVerified(account, 'recipient')).toBe(true);
  expect(await sharing.recipientVerified(account, 'other-user')).toBe(false);
  await sharing.syncDeviceGrant('share', { ...row, targetUserId: 'other-user' });
  expect(state.approve.mock.calls[0]![0].recipientVerified).toBe(false);
});
it('uses the dialog snapshot even when the pre-edit portal row has changed its device, target and permissions', async () => {
  const before = sharing.grantForRow('share', row, account, 'acl');
  state.request.mockImplementation(async (path: string) => {
    if (path === 'relay/portal') return { sharedByMe: [{ ...row, deviceId: 'substituted', targetUserId: 'attacker', threadAccess: 'control', workspaceAccess: 'write' }] };
    if (path === 'api/transport/grants') return { grants: [before] };
    if (path.endsWith('/recipient-account')) return account;
    return {};
  });
  await sharing.sharingMutation('share', 'edit', row.id, async () => ({ ...row, threadAccess: 'control' }), {
    ...sharing.sharingEditInput('share', row), threadAccess: 'control',
  });
  const put = state.request.mock.calls.find(c => c[1]?.method === 'PUT')!;
  expect(put[2]).toEqual({ deviceId: row.deviceId });
  expect(JSON.parse(put[1].body)).toEqual({ ...before, threadAccess: 'control' });
  expect(state.approve.mock.calls[0]![0].description).not.toContain('attacker');
});
it('refuses an invite that conflicts with an existing TOFU owner pin until explicit forget', async () => {
  state.account.mockResolvedValue(null);
  state.store.set('tofu-owner:device', vectors.otherAccount.accountKeyId);
  await expect(trust.pinInviteOwner('device', account.accountKeyId)).rejects.toMatchObject({ code: 'account_mismatch' });
  expect(await trust.inviteOwnerPin('device')).toBeUndefined();
  await trust.forgetInviteOwner('device'); await trust.pinInviteOwner('device', account.accountKeyId);
  expect(await trust.inviteOwnerPin('device')).toBe(account.accountKeyId);
});
