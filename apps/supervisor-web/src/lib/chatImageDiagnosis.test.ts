import { webcrypto } from 'node:crypto';
import { afterEach, expect, it, vi } from 'vitest';
import { diagnoseChatImage } from './chatImageDiagnosis';

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it.each([
  [401, 'application/json', '{}', 'Session expired'],
  [404, 'application/json', '{}', 'File not found'],
  [400, 'application/json', '{"code":"bad_request","message":"private-file: No such file or directory (os error 2)"}', 'File not found'],
  [502, 'application/json', '{"code":"device_offline"}', 'Device offline'],
  [200, 'image/png', 'not decodable', 'Image could not be displayed'],
])('probes the same URL and reports only safe metadata (%s)', async (status, type, body, reason) => {
  vi.stubGlobal('crypto', webcrypto);
  const probe = vi.fn(async () => new Response(body, { status, headers: { 'content-type': type } }));
  vi.stubGlobal('fetch', probe);
  const log = vi.spyOn(console, 'info').mockImplementation(() => {});
  const src = '/api/threads/private-thread/assets/image?path=private-file.png&token=private-token&_imageRetry=1';
  expect(await diagnoseChatImage(src)).toBe(reason);
  expect(probe).toHaveBeenCalledWith(new URL(src, location.href), expect.objectContaining({ credentials: 'include', cache: 'no-store' }));
  expect(log).toHaveBeenCalledExactlyOnceWith('chat-image-failure', { status, contentType: type, pathHash: expect.stringMatching(/^[a-f0-9]{64}$/) });
  expect(JSON.stringify(log.mock.calls)).not.toMatch(/private|localhost|https?:/);
});
it('does not probe external images or local blob previews', async () => {
  const probe = vi.fn(); vi.stubGlobal('fetch', probe);
  expect(await diagnoseChatImage('https://external.example/photo.png')).toBeNull();
  expect(await diagnoseChatImage('blob:http://localhost/local-preview')).toBeNull();
  expect(probe).not.toHaveBeenCalled();
});
it('records a failed network read without logging its exception', async () => {
  vi.stubGlobal('crypto', webcrypto);
  vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('private address and path'); }));
  const log = vi.spyOn(console, 'info').mockImplementation(() => {});
  expect(await diagnoseChatImage('/api/threads/t/assets/image?path=a.png')).toBeNull();
  expect(log).toHaveBeenCalledExactlyOnceWith('chat-image-failure', { status: 0, contentType: '', pathHash: expect.any(String) });
});
