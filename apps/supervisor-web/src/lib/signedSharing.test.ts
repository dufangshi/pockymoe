// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  RelayAccessGrantDto,
  RelaySessionShareDto,
} from '@pockymoe/shared';
import vectors from './fixtures/accountKeyVectors.json';
const state = vi.hoisted(() => ({
  request: vi.fn(),
  paired: vi.fn(),
  certified: vi.fn(),
  approve: vi.fn(),
  safety: vi.fn(),
  store: new Map<string, unknown>(),
}));
vi.mock('./api', () => ({ request: state.request }));
vi.mock('./sharingApproval', () => ({ requestGrantReview: state.approve }));
vi.mock('./sharedDeviceTrust', async (original) => ({ ...await original<object>(), isProtectedSharingDevice: state.paired, sharingGrantDevices: state.certified }));
vi.mock('./accountVault', () => ({ safetyCode: state.safety }));
vi.mock('./sharingTrustStore', () => ({
  sharingTrustStore: async (key: string, ...write: unknown[]) => {
    if (write.length) state.store.set(key, write[0]);
    else return state.store.get(key);
  },
}));
import {
  safetyCode,
  grantForRow,
  recipientVerified,
  confirmRecipient,
  reconcileGrants,
  sharingMutation,
  syncDeviceGrant,
  sharingEditInput,
} from './signedSharing';
const account = { accountKeyId: vectors.account.accountKeyId, accountPublicKey: vectors.account.accountPublicKey };
const otherAccount = vectors.otherAccount;
const row = {
  id: 'row',
  targetUserId: 'recipient',
  targetUsername: 'Recipient',
  deviceId: 'device',
  deviceConnected: true,
  threadId: 'thread',
  workspaceId: 'workspace',
  threadAccess: 'read',
  workspaceAccess: 'none',
  expiresAt: null,
} as RelaySessionShareDto;
const input = { targetIdentifier: 'Recipient', deviceId: row.deviceId, threadId: row.threadId, workspaceId: row.workspaceId, threadAccess: row.threadAccess, workspaceAccess: row.workspaceAccess };
let current: RelaySessionShareDto | RelayAccessGrantDto = row;
beforeEach(() => {
  state.store.clear();
  state.certified.mockReset().mockResolvedValue(['device']);
  state.approve.mockReset().mockReturnValue(true);
  state.safety.mockReset().mockImplementation(async (id: string) => id === account.accountKeyId ? vectors.safetyCode.display : '0000 1111 2222');
  state.request.mockReset();
  state.paired.mockReset();
  state.paired.mockResolvedValue(true);
  current = row;
  state.request.mockImplementation(async (path: string) => {
    if (path === '/relay/portal')
      return { sharedByMe: [current], grantsByMe: [current] };
    if (path.endsWith('/recipient-account')) return account;
    if (path === '/api/transport/grants') return { grants: [] };
    return {};
  });
});
describe('signed grant sharing', () => {
  it('uses the v2 Crockford safety vector from the account vault helper', async () => {
    expect(await safetyCode(vectors.safetyCode.accountKeyId)).toBe('VDNZ DB8Z 3HKC');
    expect(state.safety).toHaveBeenCalledWith(vectors.account.accountKeyId);
  });
  it('confirms only a validated account key and requires confirmation again when the recipient key changes', async () => {
    expect(await recipientVerified(account, 'recipient')).toBe(false);
    await confirmRecipient(account, 'recipient');
    expect(await recipientVerified({ ...account }, 'recipient')).toBe(true);
    expect(await recipientVerified(otherAccount, 'recipient')).toBe(false);
    await expect(recipientVerified({ ...account, accountPublicKey: otherAccount.accountPublicKey }, 'recipient')).rejects.toMatchObject({ code: 'account_mismatch' });
  });
  it('creates the relay row first, fetches the recipient, then writes the exact device ACL', async () => {
    const mutate = vi.fn(async () => ({ id: row.id }));
    await sharingMutation('share', 'create', undefined, mutate, input);
    expect(mutate).toHaveBeenCalledOnce();
    const put = state.request.mock.calls.find((c) => c[1]?.method === 'PUT')!;
    const grant = JSON.parse(put[1].body);
    expect(put[0]).toBe('/api/transport/grants/' + grant.grantId);
    expect(put[2]).toEqual({ deviceId: 'device' });
    expect(grant).toEqual(
      grantForRow('share', row, account, grant.grantId),
    );
    expect(grant.grantId).toMatch(/^[A-Za-z0-9_-]{22}$/);
  });
  it('preserves grant IDs on edits and maps device workspace selection and expiration', async () => {
    const deviceRow = {
      ...row,
      scope: 'device',
      workspaceScope: 'selected',
      workspaceIds: ['workspace'],
      canCreateThreads: true,
      expiresAt: '2026-10-15T00:00:00Z',
    } as unknown as RelayAccessGrantDto;
    const existing = grantForRow('grant', deviceRow, account, 'existing');
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, ...args) =>
      path === '/api/transport/grants'
        ? { grants: [existing] }
        : server(path, ...args),
    );
    const saved = await syncDeviceGrant('grant', deviceRow);
    expect(saved).toMatchObject({
      grantId: 'existing',
      scope: 'device',
      workspaceScope: 'selected',
      workspaceIds: ['workspace'],
      canCreateThreads: true,
      expiresAt: Date.parse(deviceRow.expiresAt!),
    });
  });
  it.each(['share', 'grant'] as const)(
    'rolls back a new %s row on device failure and requires a recipient account key',
    async (kind) => {
      const server = state.request.getMockImplementation()!;
      state.request.mockImplementation(async (path, ...args) =>
        path.endsWith('/recipient-account')
          ? Promise.reject({ code: 'recipient_account_missing' })
          : server(path, ...args),
      );
      await expect(
        sharingMutation(kind, 'create', undefined, async () => ({
          id: row.id,
        }), input),
      ).rejects.toThrow(/rolled back/);
      expect(state.request).toHaveBeenLastCalledWith(
        `/relay/${kind === 'share' ? 'shares' : 'grants'}/row`,
        { method: 'DELETE' },
      );
      expect(state.request.mock.calls.some((c) => c[1]?.method === 'PUT')).toBe(
        false,
      );
    },
  );
  it('restores previous relay permissions when an edit cannot be applied on the device', async () => {
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, init, ...args) => {
      if (init?.method === 'PUT') throw new Error('Device offline');
      return server(path, init, ...args);
    });
    await expect(
      sharingMutation('share', 'edit', row.id, async () => {
        current = { ...row, threadAccess: 'control' };
        return {};
      }, { ...sharingEditInput('share', row), threadAccess: 'control' }),
    ).rejects.toThrow(/Device offline/);
    expect(state.request).toHaveBeenLastCalledWith('/relay/shares/row', {
      method: 'PATCH',
      body: JSON.stringify(row),
    });
  });
  it('reports rollback failures without claiming the relay row was restored', async () => {
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, init, ...args) => {
      if (init?.method === 'PUT' || init?.method === 'DELETE')
        throw new Error('Failed');
      return server(path, init, ...args);
    });
    await expect(
      sharingMutation('share', 'create', undefined, async () => ({
        id: row.id,
      }), input),
    ).rejects.toThrow(/rollback failed/);
  });
  it('revokes all matching device grants before removing the relay row, and keeps relay access on device failure', async () => {
    const grant = grantForRow('share', row, account, 'grant');
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, ...args) =>
      path === '/api/transport/grants'
        ? { grants: [grant] }
        : server(path, ...args),
    );
    const mutate = vi.fn(async () => ({}));
    await sharingMutation('share', 'revoke', row.id, mutate);
    expect(state.request).toHaveBeenLastCalledWith(
      '/api/transport/grants/grant',
      { method: 'DELETE' },
      { deviceId: 'device' },
    );
    expect(mutate).toHaveBeenCalledOnce();
    mutate.mockClear();
    state.request.mockImplementation(async (path, init, ...args) => {
      if (init?.method === 'DELETE') throw new Error('Offline');
      if (path === '/api/transport/grants') return { grants: [grant] };
      return server(path, init, ...args);
    });
    await expect(
      sharingMutation('share', 'revoke', row.id, mutate),
    ).rejects.toThrow('Offline');
    expect(mutate).not.toHaveBeenCalled();
  });
  it('keeps unprotected shares as relay-only, including offline devices, and explains protected offline writes', async () => {
    state.paired.mockResolvedValue(false);
    current = { ...row, deviceConnected: false };
    await sharingMutation('share', 'create', undefined, async () => ({
      id: row.id,
    }), input);
    expect(state.request.mock.calls.map((c) => c[0])).toEqual(['/relay/portal']);
    state.paired.mockResolvedValue(true);
    state.request.mockImplementation(async (path) => {
      if (path === '/relay/portal') return { sharedByMe: [current] };
      if (path.endsWith('/recipient-account')) return account;
      if (path === '/api/transport/grants') throw { statusCode: 503 };
      return {};
    });
    await expect(
      sharingMutation('share', 'create', undefined, async () => ({
        id: row.id,
      }), input),
    ).rejects.toThrow(/protected device must be online/);
  });
  it('reconciles missing, orphan, updated account keys and unchanged grants by relayRef kind and id', () => {
    const active = grantForRow('share', row, account, 'active');
    const changed = {
      ...active,
      grantId: 'changed',
      relayRef: { kind: 'grant' as const, id: 'row' },
      grantee: { ...active.grantee, ...otherAccount },
    };
    const orphan = {
      ...active,
      grantId: 'orphan',
      relayRef: { kind: 'share' as const, id: 'absent' },
    };
    expect(
      reconcileGrants(
        [
          { kind: 'share', row, account },
          { kind: 'grant', row, account },
          { kind: 'share', row: { ...row, id: 'missing' }, account },
        ],
        [active, changed, orphan],
      ).map((e) => e.state),
    ).toEqual(['active', 'changed', 'missing', 'orphan']);
  });
  it('does not label expired device grants as still active', () => {
    const expired = {
      ...grantForRow('share', row, account, 'expired'),
      expiresAt: Date.now() - 1,
    };
    expect(reconcileGrants([], [expired])).toEqual([]);
    expect(
      reconcileGrants([{ kind: 'share', row, account }], [expired])[0]
        ?.state,
    ).toBe('missing');
  });

  it('explains that protected devices must be online even when relay presence is stale', async () => {
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, ...args) => {
      if (path === '/api/transport/grants') throw { statusCode: 503 };
      return server(path, ...args);
    });
    await expect(
      sharingMutation('share', 'create', undefined, async () => ({
        id: row.id,
      }), input),
    ).rejects.toThrow(/protected device must be online/);
  });
  it('flags permission drift after an edit response is lost and its relay row rolls back', () => {
    const grant = {
      ...grantForRow('share', row, account, 'grant'),
      threadAccess: 'control' as const,
    };
    expect(
      reconcileGrants([{ kind: 'share', row, account }], [grant])[0]?.state,
    ).toBe('different');
  });
});

// A compromised portal may alter every signed field. Only the dialog snapshot
// supplies new grant permissions; rejection must precede a device PUT.
it.each([
  { deviceId: 'another-device' },
  { threadId: 'another-thread' },
  { workspaceId: 'another-workspace' },
  { threadAccess: 'control' },
  { workspaceAccess: 'write' },
  { expiresAt: '2027-01-01T00:00:00Z' },
])('refuses and rolls back a share row altered by the relay: %j', async (altered) => {
  current = { ...row, ...altered } as RelaySessionShareDto;
  await expect(sharingMutation('share', 'create', undefined, async () => ({ id: row.id }), input)).rejects.toThrow(/differ from your choices/);
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
  expect(state.approve).not.toHaveBeenCalled();
  expect(state.request).toHaveBeenLastCalledWith('/relay/shares/row', { method: 'DELETE' });
});
it.each([
  { scope: 'device' }, { workspaceScope: 'selected' },
  { workspaceIds: ['secret'] }, { canCreateThreads: true },
])('refuses a relay-altered grant field: %j', async (altered) => {
  const chosen = { ...input, scope: 'thread' as const };
  current = { ...row, scope: 'thread', workspaceScope: 'all', workspaceIds: [], canCreateThreads: false, ...altered } as unknown as RelayAccessGrantDto;
  await expect(sharingMutation('grant', 'create', undefined, async () => ({ id: row.id }), chosen)).rejects.toThrow(/differ from your choices/);
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
});
it('requires explicit exact-field and unverified-recipient acknowledgement before reconciliation signing', async () => {
  state.approve.mockReturnValue(false);
  await expect(syncDeviceGrant('share', row)).rejects.toThrow(/cancelled/);
  const review = state.approve.mock.calls[0]![0].description;
  expect(state.approve.mock.calls[0]![0].recipientVerified).toBe(false);
  for (const value of ['device', 'thread', 'workspace', 'read', 'none', 'No expiry', 'Recipient', 'recipient', await safetyCode(account.accountKeyId)])
    expect(review).toContain(value);
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
  state.approve.mockReturnValue(true);
  await syncDeviceGrant('share', row);
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(true);
  expect(await recipientVerified(account, 'recipient')).toBe(false); // acknowledgement does not claim verification
});
it('still requires exact-field review for an already verified account key', async () => {
  await confirmRecipient(account, 'recipient');
  await syncDeviceGrant('share', row);
  expect(state.approve).toHaveBeenCalledOnce();
  expect(state.approve.mock.calls[0]![0].recipientVerified).toBe(true);
});
it('checks every certified device even when the portal hides or reassigns the row', async () => {
  state.certified.mockResolvedValue(['hidden-device', 'other-device']);
  const grant = grantForRow('share', row, account, 'hidden');
  state.request.mockImplementation(async (path, _init, options) => {
    if (path === '/relay/portal') throw new Error('Portal must not supply revocation identity');
    if (path === '/api/transport/grants') return { grants: options.deviceId === 'hidden-device' ? [grant] : [] };
    return {};
  });
  const mutate = vi.fn(async () => ({}));
  await sharingMutation('share', 'revoke', row.id, mutate);
  expect(state.request).toHaveBeenCalledWith('/api/transport/grants/hidden', { method: 'DELETE' }, { deviceId: 'hidden-device' });
  expect(state.request).toHaveBeenCalledWith('/api/transport/grants', undefined, { deviceId: 'other-device' });
  expect(mutate).toHaveBeenCalledOnce();
});
it('keeps the relay row if any certified device is offline, even when no grant is visible elsewhere', async () => {
  state.certified.mockResolvedValue(['visible', 'hidden-offline']);
  state.request.mockImplementation(async (_path, _init, options) => {
    if (options.deviceId === 'hidden-offline') throw { statusCode: 503 };
    return { grants: [] };
  });
  const mutate = vi.fn(async () => ({}));
  await expect(sharingMutation('share', 'revoke', row.id, mutate)).rejects.toThrow(/protected device must be online/);
  expect(mutate).not.toHaveBeenCalled();
  expect(state.request.mock.calls.some(c => c[1]?.method === 'DELETE')).toBe(false);
});
it('does not sign grants when automatic title metadata is refreshed', async () => {
  const mutate = vi.fn(async () => ({}));
  await sharingMutation('share', 'edit', row.id, mutate, { threadTitle: 'Updated title' });
  expect(mutate).toHaveBeenCalledOnce();
  expect(state.request).not.toHaveBeenCalled();
  expect(state.approve).not.toHaveBeenCalled();
});

it('uses only dialog permissions even when a full mutation reply is followed by an altered portal', async () => {
  current = { ...row, deviceId: 'relay-selected', threadAccess: 'control' };
  await sharingMutation('share', 'create', undefined, async () => ({ ...row }), input);
  expect(state.request).not.toHaveBeenCalledWith('/relay/portal');
  const put = state.request.mock.calls.find(c => c[1]?.method === 'PUT')!;
  expect(put[2].deviceId).toBe('device');
  expect(JSON.parse(put[1].body)).toMatchObject({ scope: 'thread', threadId: 'thread', threadAccess: 'read', workspaceAccess: 'none', canCreateThreads: false });
});
it('refuses a relay edit reply that reassigns the device or recipient', async () => {
  for (const altered of [{ deviceId: 'other-device' }, { targetUserId: 'relay-user' }]) {
    current = row;
    await expect(sharingMutation('share', 'edit', row.id, async () => ({ ...row, ...altered, threadAccess: 'control' }), { ...sharingEditInput('share', row), threadAccess: 'control' })).rejects.toThrow(/differ from your choices/);
  }
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
});
it('rolls back a new row if the owner cancels first-grant acknowledgement', async () => {
  state.approve.mockReturnValue(false);
  await expect(sharingMutation('share', 'create', undefined, async () => ({ ...row }), input)).rejects.toThrow(/cancelled/);
  expect(state.request).toHaveBeenLastCalledWith('/relay/shares/row', { method: 'DELETE' });
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
});
it('reviews the current account key again instead of reusing a stale verified status', async () => {
  await confirmRecipient(account, 'recipient');
  const changed = otherAccount;
  const server = state.request.getMockImplementation()!;
  state.request.mockImplementation(async (path, ...args) => path.endsWith('/recipient-account') ? changed : server(path, ...args));
  state.approve.mockReturnValue(false);
  await expect(syncDeviceGrant('share', row)).rejects.toThrow(/cancelled/);
  expect(state.approve.mock.calls[0]![0]).toMatchObject({ recipientVerified: false });
  expect(state.approve.mock.calls[0]![0].description).toContain(await safetyCode(changed.accountKeyId));
  expect(state.request.mock.calls.some(c => c[1]?.method === 'PUT')).toBe(false);
});
