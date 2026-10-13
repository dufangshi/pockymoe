import { beforeEach, expect, it, vi } from 'vitest';
import { request, ApiError } from './api';
import { getTransportStatus } from './relayTransport';
import { openDevicePortPreview } from './portPreview';
vi.mock('./api', () => ({ request: vi.fn(), ApiError: class extends Error { constructor(public statusCode: number, _message?: unknown) { super('failed'); } } }));
vi.mock('./relayTransport', () => ({ getTransportStatus: vi.fn() }));
beforeEach(() => { vi.resetAllMocks(); });
it('obtains a signed device lease before opening a relay preview with query lease', async () => {
  vi.mocked(request).mockResolvedValueOnce({ leaseId: 'lease-id', expiresAt: Date.now() + 10000 }).mockResolvedValueOnce({ url: 'https://preview.invalid/' });
  expect(await openDevicePortPreview('device', 'mapping', '/a?q=1#b')).toEqual({ url: 'https://preview.invalid/' });
  expect(request).toHaveBeenNthCalledWith(1, 'relay/devices/device/api/port-mappings/mapping/lease', expect.objectContaining({ method: 'POST' }));
  expect(request).toHaveBeenNthCalledWith(2, 'relay/devices/device/port-mappings/mapping/open?lease=lease-id', expect.objectContaining({ method: 'POST', body: JSON.stringify({ path: '/a?q=1#b' }) }));
});
it('never opens a preview after a failed or invalid lease on a protected device', async () => {
  vi.mocked(getTransportStatus).mockReturnValue({ deviceId: 'device', state: 'encrypted', paired: true });
  vi.mocked(request).mockRejectedValueOnce(new ApiError(404, { code: 'not_found', message: 'failed' }));
  await expect(openDevicePortPreview('device', 'mapping', '/')).rejects.toThrow();
  expect(request).toHaveBeenCalledTimes(1);
  vi.mocked(request).mockResolvedValueOnce({ leaseId: '', expiresAt: 0 });
  await expect(openDevicePortPreview('device', 'mapping', '/')).rejects.toThrow();
  expect(request).toHaveBeenCalledTimes(2);
});
it('keeps older explicitly unpaired devices working when they have no lease endpoint', async () => {
  vi.mocked(getTransportStatus).mockReturnValue({ deviceId: 'device', state: 'legacy', paired: false });
  vi.mocked(request).mockRejectedValueOnce(new ApiError(404, { code: 'not_found', message: 'failed' })).mockResolvedValueOnce({ url: 'https://preview.invalid/' });
  await openDevicePortPreview('device', 'mapping', '/');
  expect(request).toHaveBeenLastCalledWith('relay/devices/device/port-mappings/mapping/open', expect.anything());
});

it.each([undefined, Date.now() + 100000])('reuses a supplied CLI or DSH lease with expiry %s', async expiresAt => {
  vi.mocked(request).mockResolvedValueOnce({ url: 'https://preview.invalid/' });
  await openDevicePortPreview('device', 'mapping', '/console?q=1', undefined, { leaseId: 'existing_lease', expiresAt });
  expect(request).toHaveBeenCalledTimes(1);
  expect(request).toHaveBeenCalledWith('relay/devices/device/port-mappings/mapping/open?lease=existing_lease', expect.objectContaining({ body: JSON.stringify({ path: '/console?q=1' }) }));
});
it('renews a known-expired supplied lease before opening the preview', async () => {
  vi.mocked(request).mockResolvedValueOnce({ leaseId: 'renewed', expiresAt: Date.now() + 10000 }).mockResolvedValueOnce({ url: 'https://preview.invalid/' });
  await openDevicePortPreview('device', 'mapping', '/', undefined, { leaseId: 'expired', expiresAt: Date.now() - 1 });
  expect(request).toHaveBeenCalledTimes(2);
  expect(request).toHaveBeenLastCalledWith('relay/devices/device/port-mappings/mapping/open?lease=renewed', expect.anything());
});
