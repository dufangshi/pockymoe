import { beforeEach, expect, it, vi } from 'vitest';
import { fetchRelaySession } from './api';
import { setAccountSession } from './accountAuth';
vi.mock('./accountAuth', () => ({ setAccountSession: vi.fn(), startAccountLogin: vi.fn(), cancelAccountLogin: vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); localStorage.setItem('remote-codex-relay-mode', 'true'); });
it('sets the account context even when the relay labels a vault user admin', async () => {
  const account = { mode: 'account', setupRequired: false, recovery: false, hasVault: true, accountKeyId: 'known-key' };
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ authenticated: true, user: { id: 'known-user', role: 'admin' }, account })));
  try {
    await fetchRelaySession();
    expect(setAccountSession).toHaveBeenCalledWith('known-user', account);
  } finally { vi.unstubAllGlobals(); localStorage.clear(); }
});
