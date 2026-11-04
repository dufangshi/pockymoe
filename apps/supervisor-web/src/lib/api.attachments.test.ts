import { afterEach, expect, it, vi } from 'vitest';
import { buildThreadImageAssetUrl, downloadThreadImage, recoverChatImage, setSelectedRelayDeviceId, refreshChatImageUrl, setStoredAuthToken, connectSupervisorEvents } from './api';
import { encryptedBrowserFetch, ensureRelayWorker } from './relayTransport';
vi.mock('./relayTransport', () => ({ encryptedBrowserFetch: vi.fn(), encryptedRelaySocket: vi.fn(), ensureRelayWorker: vi.fn() }));
afterEach(() => { localStorage.clear(); vi.unstubAllGlobals(); });
it('downloads shared images through encrypted transport without relying on a service worker', async () => {
  localStorage.setItem('remote-codex-relay-mode', 'true');
  setSelectedRelayDeviceId('mac');
  const plain = vi.fn();
  vi.stubGlobal('fetch', plain);
  vi.mocked(encryptedBrowserFetch).mockResolvedValue(new Response('image', { headers: { 'content-type': 'image/png' } }));
  const blob = await downloadThreadImage('thread', './photo.png');
  expect(encryptedBrowserFetch).toHaveBeenCalledWith('/relay/devices/mac/api/threads/thread/assets/image?path=.%2Fphoto.png', expect.objectContaining({ credentials: 'same-origin' }), undefined);
  expect(blob.size).toBe(5);
  expect(plain).not.toHaveBeenCalled();
});
it('recovers a failed relay image through page transport into an owned blob URL', async () => {
  localStorage.setItem('remote-codex-relay-mode', 'true');
  vi.mocked(encryptedBrowserFetch).mockResolvedValue(new Response('image', { headers: { 'content-type': 'image/png' } }));
  const url = await recoverChatImage('/relay/devices/mac/api/threads/thread/assets/image?path=./photo.png', new AbortController().signal);
  expect(url?.size).toBe(5);
  expect(encryptedBrowserFetch).toHaveBeenCalledWith(expect.stringContaining('/relay/devices/mac/api/threads/thread/assets/image'), expect.objectContaining({ credentials: 'include', cache: 'no-store' }), undefined, { registerWorker: false });
});
it('asks for the thumbnail variant only when requested', () => {
  expect(buildThreadImageAssetUrl('t 1', { path: './a.jpg' })).toBe('/api/threads/t%201/assets/image?path=.%2Fa.jpg');
  expect(buildThreadImageAssetUrl('t 1', { path: './a.jpg', variant: 'thumbnail' })).toBe('/api/threads/t%201/assets/image?path=.%2Fa.jpg&variant=thumbnail');
});

it('re-resolves local authorization and leaves external images alone', async () => {
  setStoredAuthToken('fixture-current');
  const fresh = new URL(await refreshChatImageUrl('/api/threads/t/assets/image?path=a.png&token=fixture-old'));
  expect(fresh.searchParams.get('token')).toBe('fixture-current');
  expect(await refreshChatImageUrl('https://external.example/api/threads/t/assets/image?path=a.png')).toBe('https://external.example/api/threads/t/assets/image?path=a.png');
});
it('reinitializes relay media routing on recovery, keeping the original device and using cookies', async () => {
  localStorage.setItem('remote-codex-relay-mode', 'true');
  setSelectedRelayDeviceId('another-device');
  const fresh = new URL(await refreshChatImageUrl('/relay/devices/original/api/threads/t/assets/image?path=a.png'));
  expect(ensureRelayWorker).toHaveBeenCalled();
  expect(fresh.pathname).toContain('/devices/original/');
  expect(fresh.searchParams.has('relaySession')).toBe(false);
  expect(fresh.searchParams.has('token')).toBe(false);
});

it('announces a fresh live connection so mounted failed images can recover', () => {
  class Socket extends EventTarget { constructor(_url: string) { super(); } }
  vi.stubGlobal('WebSocket', Socket);
  const recovered = vi.fn();
  window.addEventListener('pockymoe-images-reconnected', recovered);
  try {
    const socket = connectSupervisorEvents(() => {});
    expect(recovered).not.toHaveBeenCalled();
    socket.dispatchEvent(new Event('open'));
    expect(recovered).toHaveBeenCalledTimes(1);
  } finally { window.removeEventListener('pockymoe-images-reconnected', recovered); }
});

it('never gives the image fallback credentials for external URLs or non-image APIs', async () => {
  const before = vi.mocked(encryptedBrowserFetch).mock.calls.length;
  for (const src of ['https://external.example/api/threads/t/assets/image?path=a.png', '/relay/devices/d/api/workspaces', 'blob:expired']) {
    expect(await recoverChatImage(src, new AbortController().signal)).toBeNull();
  }
  expect(encryptedBrowserFetch).toHaveBeenCalledTimes(before);
});
