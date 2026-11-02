// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { request } from '../lib/api';
import { clearMobilePush, disableMobilePush, mobilePushEnabled, registerMobilePush, restoreMobilePush, setLiveActivityToken } from './push';
vi.mock('../lib/api', () => ({ request: vi.fn() }));
const register = vi.fn();
vi.mock('../desktop/surface', () => ({ mobileHost: () => ({ push: { register } }) }));
afterEach(() => { clearMobilePush(); localStorage.clear(); vi.resetAllMocks(); });
describe('mobile push registration', () => {
  const settings = { publicKey: '', subscriptionIds: ['registration'] , scope: 'all' };
  it('registers only explicit native tokens and restores its non-secret id after reload', async () => {
    restoreMobilePush('account', settings);
    await setLiveActivityToken('live-token'); expect(request).not.toHaveBeenCalled();
    register.mockResolvedValue({ token: 'device-token', sandbox: true, kind: 'ios' });
    vi.mocked(request).mockResolvedValue({ id: 'registration' });
    await registerMobilePush();
    expect(request).toHaveBeenCalledWith('relay/account/notifications/native', expect.objectContaining({ method: 'POST', body: expect.stringContaining('"liveActivityToken":"live-token"') }));
    expect(JSON.stringify(localStorage)).not.toContain('device-token'); expect(JSON.stringify(localStorage)).not.toContain('live-token');
    clearMobilePush(); expect(restoreMobilePush('account', settings)).toBe(true); expect(mobilePushEnabled(settings)).toBe(true);
    expect(restoreMobilePush('another-account', settings)).toBe(false);
    restoreMobilePush('account', settings); await disableMobilePush();
    expect(request).toHaveBeenLastCalledWith('relay/account/notifications/native', { method: 'DELETE', body: '{"id":"registration"}' });
    expect(mobilePushEnabled(settings)).toBe(false);
  });
  it('does not enable another account when permission returns after an account switch', async () => {
    restoreMobilePush('account', settings);
    let resolve!: (value: unknown) => void;
    register.mockImplementation(() => new Promise(r => { resolve = r; }));
    const pending = registerMobilePush();
    restoreMobilePush('another-account', settings);
    resolve({ token: 'device-token', sandbox: true, kind: 'ios' });
    expect(await pending).toBe(null); expect(request).not.toHaveBeenCalled();
  });
  it('does not revive disabled registration on a token command', async () => {
    restoreMobilePush('account', settings);
    await setLiveActivityToken('token'); expect(register).not.toHaveBeenCalled(); expect(request).not.toHaveBeenCalled();
  });
});
