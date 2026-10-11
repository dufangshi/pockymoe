import { afterEach, expect, it, vi } from 'vitest';
import { buildThreadImageAssetUrl, downloadThreadImage, setSelectedRelayDeviceId } from './api';
import { encryptedBrowserFetch } from './relayTransport';
vi.mock('./relayTransport', () => ({ encryptedBrowserFetch: vi.fn(), encryptedRelaySocket: vi.fn() }));
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
it('asks for the thumbnail variant only when requested', () => {
  expect(buildThreadImageAssetUrl('t 1', { path: './a.jpg' })).toBe('/api/threads/t%201/assets/image?path=.%2Fa.jpg');
  expect(buildThreadImageAssetUrl('t 1', { path: './a.jpg', variant: 'thumbnail' })).toBe('/api/threads/t%201/assets/image?path=.%2Fa.jpg&variant=thumbnail');
});
