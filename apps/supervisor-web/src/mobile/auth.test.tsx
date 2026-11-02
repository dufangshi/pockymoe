// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { request } from '../lib/api';
import { OAuthButtons } from '../components/auth/AuthUi';
import { fireEvent, render, screen, cleanup } from '@testing-library/react';
import { openOAuth } from './auth';
import { connectedMobileHost, initializeMobileConnection } from './connection';
vi.mock('../lib/api', () => ({ request: vi.fn(), enableRelayMode: vi.fn(), startOAuthSignUp: vi.fn() }));
vi.mock('./connection', () => ({ connectedMobileHost: vi.fn(), initializeMobileConnection: vi.fn(async () => {}) }));
vi.mock('../desktop/surface', () => ({ useHostSurface: () => 'mobile' }));
vi.stubGlobal('crypto', webcrypto);
afterEach(() => { cleanup(); vi.clearAllMocks(); });
it('uses the callback-only code plus its page proof, never the known preparation id', async () => {
  const external = vi.fn(async () => ({ callbackUrl: `remotecodex://auth/callback?code=${'c'.repeat(43)}` }));
  vi.mocked(connectedMobileHost).mockReturnValue({ auth: { openExternal: external } } as never);
  vi.mocked(request).mockResolvedValueOnce({ path: '/relay/auth/oauth/google/start?mobileId=public-id', callbackUrl: 'remotecodex://auth/callback' }).mockRejectedValueOnce(new Error('exchange observed'));
  await expect(openOAuth('google')).rejects.toThrow('exchange observed');
  const body = JSON.parse(vi.mocked(request).mock.calls[1]![1]!.body as string);
  expect(body).toEqual({ code: 'c'.repeat(43), proof: expect.stringMatching(/^[a-f0-9]{64}$/) });
  expect(body).not.toHaveProperty('id'); expect(external).toHaveBeenCalled();
});
it.each([null, { callbackUrl: 'remotecodex://auth/callback?id=public-id' }, { callbackUrl: `evil://auth/callback?code=${'c'.repeat(43)}` }])('shows failure instead of silently returning on a cancelled or invalid callback', async result => {
  vi.mocked(connectedMobileHost).mockReturnValue({ auth: { openExternal: async () => result } } as never);
  vi.mocked(request).mockResolvedValue({ path: '/relay/auth/oauth/github/start?mobileId=public-id', callbackUrl: 'remotecodex://auth/callback' });
  render(<OAuthButtons github />);
  fireEvent.click(screen.getByRole('button'));
  await vi.waitFor(() => expect(document.querySelector('.auth-notice[data-tone="warning"]')).toBeTruthy());
  expect(request).toHaveBeenCalledTimes(1);
});
it('shows a failed connection rather than attempting sign-in against the Web origin', async () => {
  vi.mocked(initializeMobileConnection).mockRejectedValueOnce(new Error('Unable to connect'));
  await expect(openOAuth('google')).rejects.toThrow('Unable to connect'); expect(request).not.toHaveBeenCalled();
});

it.each(['/relay/auth/oauth/github/start?mobileId=public-id', 'https://other.example.com/relay/auth/oauth/google/start?mobileId=public-id'])('rejects an unexpected system-auth start target: %s', async path => {
  const external = vi.fn();
  vi.mocked(connectedMobileHost).mockReturnValue({ auth: { openExternal: external } } as never);
  vi.mocked(request).mockResolvedValue({ path, callbackUrl: 'remotecodex://auth/callback' });
  await expect(openOAuth('google')).rejects.toThrow();
  expect(external).not.toHaveBeenCalled();
});
