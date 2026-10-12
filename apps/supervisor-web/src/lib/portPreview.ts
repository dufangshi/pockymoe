import { translate } from '@pockymoe/thread-ui/i18n';
import { request, ApiError } from './api';
import { getTransportStatus } from './relayTransport';

/** Device authorization precedes the relay's preview-host login handoff. */
export async function openDevicePortPreview(deviceId: string, mappingId: string, path: string, signal?: AbortSignal) {
  const device = `/relay/devices/${encodeURIComponent(deviceId)}`;
  let leaseId: string | undefined;
  try {
    const lease = await request<{ leaseId: string; expiresAt: number }>(`${device}/api/port-mappings/${encodeURIComponent(mappingId)}/lease`, { method: 'POST', ...(signal ? { signal } : {}) });
    if (!lease.leaseId || !Number.isSafeInteger(lease.expiresAt) || lease.expiresAt <= Date.now()) throw new Error(translate('devices.portMappingFailed'));
    leaseId = lease.leaseId;
  } catch (error) {
    const status = getTransportStatus(deviceId);
    // Old unpaired Supervisors have no lease route. A protected device must
    // never fall back on a relay-generated 404 or other lease failure.
    if (!(error instanceof ApiError && error.statusCode === 404 && status?.paired === false && ['legacy', 'encrypted'].includes(status.state))) throw error;
  }
  return request<{ url: string }>(`${device}/port-mappings/${encodeURIComponent(mappingId)}/open${leaseId ? `?lease=${encodeURIComponent(leaseId)}` : ''}`, {
    method: 'POST', ...(signal ? { signal } : {}), body: JSON.stringify({ path }),
  });
}
