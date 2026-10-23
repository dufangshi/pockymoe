import { afterEach, expect, it, vi } from 'vitest';
import { request } from './api';
import { normalizeSafetyCode, onSharePending, pendingShare, sharePendingHref } from './sharePending';

afterEach(() => vi.unstubAllGlobals());

// The relay's refusal, as `share_verification::refusal` sends it.
const refusal = {
  code: 'share_pending',
  message: 'owner has not confirmed your safety code yet, so this shared access has not started.',
  share: {
    kind: 'grant', id: 'g1', scope: 'workspace', ownerUsername: 'owner', deviceId: 'dev', deviceName: 'Laptop',
    threadId: null, threadTitle: null, workspaceId: 'web', workspaceLabel: 'Web app', label: null,
    threadAccess: 'control', workspaceAccess: 'read',
  },
};

it('turns the relay refusal into one event naming the waiting share, and an error that says why', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(refusal, { status: 403 })));
  const seen: unknown[] = [];
  const stop = onSharePending(share => seen.push(share));
  try {
    await expect(request('api/workspaces/web/threads')).rejects.toMatchObject({
      statusCode: 403,
      payload: { code: 'share_pending', message: 'owner must confirm your safety code before you can open this.', details: { share: refusal.share } },
    });
  } finally {
    stop();
  }
  expect(seen).toEqual([refusal.share]);
  expect(sharePendingHref(refusal.share as never)).toBe('/shared-pending?kind=grant&id=g1');
});

it('ignores other refusals and malformed shares', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ code: 'forbidden', message: 'no' }, { status: 403 })));
  const listener = vi.fn();
  const stop = onSharePending(listener);
  await expect(request('api/threads/t1')).rejects.toMatchObject({ payload: { code: 'forbidden' } });
  stop();
  expect(listener).not.toHaveBeenCalled();
  expect(pendingShare({ ...refusal.share, kind: 'other' })).toBeNull();
  expect(pendingShare({ ...refusal.share, id: 7 })).toBeNull();
});

it('reads a typed code the way the relay compares it', () => {
  expect(normalizeSafetyCode(' vdnz-db8z 3hkc ')).toBe('VDNZDB8Z3HKC');
  expect(normalizeSafetyCode('O1 il')).toBe('0111');
});
