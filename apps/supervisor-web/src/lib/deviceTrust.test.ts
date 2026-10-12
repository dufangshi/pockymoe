// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { exchange } from './relayTransportCrypto';
import { deviceTrustRequest } from './deviceTrust';
vi.mock('./relayTransportCrypto', () => ({ exchange: vi.fn() }));
vi.stubGlobal('location', new URL('https://pockymoe.test'));
beforeEach(() => {
  vi.clearAllMocks();
});
describe('v2 device trust transport', () => {
  it.each([
    'enrollment_missing',
    'browser_revoked',
    'account_mismatch',
    'invalid_certificate',
  ])('preserves %s for enrollment and trust callers', async (code) => {
    vi.mocked(exchange).mockResolvedValue({
      response: Response.json({ code }, { status: 403 }),
    });
    await expect(deviceTrustRequest('d', 'enrollment')).rejects.toMatchObject({
      code,
      status: 403,
    });
    expect(vi.mocked(exchange).mock.calls[0]![0].url).toBe(
      'https://pockymoe.test/relay/devices/d/api/transport/enrollment',
    );
  });
  it('sends certificate installation through encrypted exchange', async () => {
    vi.mocked(exchange).mockResolvedValue({
      response: Response.json({ certificate: {} }),
    });
    await deviceTrustRequest('d', 'device-certificate', 'PUT', { version: 1 });
    const request = vi.mocked(exchange).mock.calls[0]![0];
    expect(request.method).toBe('PUT');
    expect(await request.json()).toEqual({ version: 1 });
    expect(request.credentials).toBe('include');
  });
});
