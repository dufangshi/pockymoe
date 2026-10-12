// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  RelayAccessGrantDto,
  RelaySessionShareDto,
} from '@pockymoe/shared';
import vectors from './fixtures/ownerTrustVectors.json';
import type { OwnerCredential } from './ownerTrustCrypto';
const state = vi.hoisted(() => ({
  request: vi.fn(),
  paired: vi.fn(),
  store: new Map<string, unknown>(),
}));
vi.mock('./api', () => ({ request: state.request }));
vi.mock('./ownerDeviceTrust', () => ({ hasDeviceCertificate: state.paired }));
vi.mock('./ownerTrustStore', () => ({
  trustStore: async (key: string, ...write: unknown[]) => {
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
} from './signedSharing';
const credentials = [vectors.credentialRecord as OwnerCredential];
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
let current = row;
beforeEach(() => {
  state.store.clear();
  state.request.mockReset();
  state.paired.mockReset();
  state.paired.mockResolvedValue(true);
  current = row;
  state.request.mockImplementation(async (path: string) => {
    if (path === '/relay/portal')
      return { sharedByMe: [current], grantsByMe: [current] };
    if (path.endsWith('/recipient-passkeys')) return { passkeys: credentials };
    if (path === '/api/transport/grants') return { grants: [] };
    return {};
  });
});
describe('signed grant sharing', () => {
  it('uses the fixed independently computed safety vector, sorting IDs and formatting twelve characters', async () => {
    const keys = [
      { credentialId: 'VAmvL72mWjfOOwlr7U2guw-_bOqfmL5xKuXeWO3sBSc' },
      { credentialId: 'AQID' },
    ];
    // SHA-256 base64url: DP_cV_wgoyVZCrM6qkRNcKKL3T2Qxv_riO7_W6JfOgk
    // Independently computed with Python hashlib over the exact domain + sorted IDs.
    expect(await safetyCode(keys)).toBe('DP_c V_wg oyVZ');
    expect(await safetyCode(keys.reverse())).toBe('DP_c V_wg oyVZ');
  });
  it('confirms only the exact credential set, including public keys, independently of relay names or ordering', async () => {
    expect(await recipientVerified(credentials)).toBe(false);
    await confirmRecipient(credentials);
    expect(
      await recipientVerified([{ ...credentials[0]!, name: 'Renamed' }]),
    ).toBe(true);
    expect(
      await recipientVerified([
        { ...credentials[0]!, publicKey: 'substituted' },
      ]),
    ).toBe(false);
    expect(
      await recipientVerified([
        ...credentials,
        { ...credentials[0]!, credentialId: 'AQID' },
      ]),
    ).toBe(false);
  });
  it('creates the relay row first, fetches the recipient, then writes the exact device ACL', async () => {
    const mutate = vi.fn(async () => ({ id: row.id }));
    await sharingMutation('share', 'create', undefined, mutate);
    expect(mutate).toHaveBeenCalledOnce();
    const put = state.request.mock.calls.find((c) => c[1]?.method === 'PUT')!;
    const grant = JSON.parse(put[1].body);
    expect(put[0]).toBe('/api/transport/grants/' + grant.grantId);
    expect(put[2]).toEqual({ deviceId: 'device' });
    expect(grant).toEqual(
      grantForRow('share', row, credentials, grant.grantId),
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
    const existing = grantForRow('grant', deviceRow, credentials, 'existing');
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
    'rolls back a new %s row on device failure and requires recipient passkeys',
    async (kind) => {
      const server = state.request.getMockImplementation()!;
      state.request.mockImplementation(async (path, ...args) =>
        path.endsWith('/recipient-passkeys')
          ? { passkeys: [] }
          : server(path, ...args),
      );
      await expect(
        sharingMutation(kind, 'create', undefined, async () => ({
          id: row.id,
        })),
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
      }),
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
      })),
    ).rejects.toThrow(/rollback failed/);
  });
  it('revokes all matching device grants before removing the relay row, and keeps relay access on device failure', async () => {
    const grant = grantForRow('share', row, credentials, 'grant');
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
  it('keeps unpaired shares as relay-only, including offline devices, and explains paired offline writes', async () => {
    state.paired.mockResolvedValue(false);
    current = { ...row, deviceConnected: false };
    await sharingMutation('share', 'create', undefined, async () => ({
      id: row.id,
    }));
    expect(state.request.mock.calls.map((c) => c[0])).toEqual([
      '/relay/portal',
    ]);
    state.paired.mockResolvedValue(true);
    await expect(
      sharingMutation('share', 'create', undefined, async () => ({
        id: row.id,
      })),
    ).rejects.toThrow(/paired device must be online/);
  });
  it('reconciles missing, orphan, updated credential sets and unchanged grants by relayRef kind and id', () => {
    const active = grantForRow('share', row, credentials, 'active');
    const changed = {
      ...active,
      grantId: 'changed',
      relayRef: { kind: 'grant' as const, id: 'row' },
      grantee: { ...active.grantee, credentials: [] },
    };
    const orphan = {
      ...active,
      grantId: 'orphan',
      relayRef: { kind: 'share' as const, id: 'absent' },
    };
    expect(
      reconcileGrants(
        [
          { kind: 'share', row, credentials },
          { kind: 'grant', row, credentials },
          { kind: 'share', row: { ...row, id: 'missing' }, credentials },
        ],
        [active, changed, orphan],
      ).map((e) => e.state),
    ).toEqual(['active', 'changed', 'missing', 'orphan']);
  });
  it('does not label expired device grants as still active', () => {
    const expired = {
      ...grantForRow('share', row, credentials, 'expired'),
      expiresAt: Date.now() - 1,
    };
    expect(reconcileGrants([], [expired])).toEqual([]);
    expect(
      reconcileGrants([{ kind: 'share', row, credentials }], [expired])[0]
        ?.state,
    ).toBe('missing');
  });

  it('explains that paired devices must be online even when relay presence is stale', async () => {
    const server = state.request.getMockImplementation()!;
    state.request.mockImplementation(async (path, ...args) => {
      if (path === '/api/transport/grants') throw { statusCode: 503 };
      return server(path, ...args);
    });
    await expect(
      sharingMutation('share', 'create', undefined, async () => ({
        id: row.id,
      })),
    ).rejects.toThrow(/paired device must be online/);
  });
  it('flags permission drift after an edit response is lost and its relay row rolls back', () => {
    const grant = {
      ...grantForRow('share', row, credentials, 'grant'),
      threadAccess: 'control' as const,
    };
    expect(
      reconcileGrants([{ kind: 'share', row, credentials }], [grant])[0]?.state,
    ).toBe('different');
  });
});
