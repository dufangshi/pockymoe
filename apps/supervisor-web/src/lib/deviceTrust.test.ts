// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { exchange } from './relayTransportCrypto';
import { deviceTrustRequest } from './deviceTrust';
vi.mock('./relayTransportCrypto', () => ({ exchange: vi.fn() }));
vi.stubGlobal('location', new URL('https://pockymoe.test'));
beforeEach(() => {
  vi.clearAllMocks();
  setLocale('en');
});
describe('device trust errors', () => {
  it.each(['en', 'zh-CN'] as const)('maps the device origin/RP mismatch to clear %s add-passkey guidance', async (locale) => {
    setLocale(locale);
    vi.mocked(exchange).mockResolvedValue({ response: Response.json({ code: 'owner_origin_mismatch' }, { status: 400 }) });
    await expect(deviceTrustRequest('d', 'credentials', 'POST', { credential: {} })).rejects.toMatchObject({
      code: 'owner_origin_mismatch', message: translate('auth.owner_origin_mismatch'),
    });
    expect(vi.mocked(exchange).mock.calls[0]![0].url).toBe('https://pockymoe.test/relay/devices/d/api/transport/credentials');
  });
  it('preserves session revocation instead of presenting a generic trust failure', async () => {
    vi.mocked(exchange).mockResolvedValue({ response: Response.json({ code: 'owner_session_revoked' }, { status: 401 }) });
    await expect(deviceTrustRequest('d')).rejects.toMatchObject({
      code: 'owner_session_revoked', message: translate('auth.owner_session_revoked'),
    });
  });
});
