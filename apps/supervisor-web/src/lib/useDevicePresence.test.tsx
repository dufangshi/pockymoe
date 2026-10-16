import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { request } from './api';
import { useDevicePresence } from './useDevicePresence';

vi.mock('./api', () => ({ request: vi.fn() }));

const portal = {
  user: {}, sharedWithMe: [], sharedByMe: [],
  devices: [
    { id: 'mac', name: 'Mac', connected: true },
    { id: 'nas', name: 'NAS', connected: true },
    { id: 'pi', name: 'Pi', connected: false },
  ],
} as unknown as RelayPortalSummaryDto;
const connected = (result: ReturnType<typeof useDevicePresence>) => result!.devices.map(device => `${device.id}:${device.connected}`);

describe('useDevicePresence', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); vi.mocked(request).mockReset(); });

  it('shows devices as the relay lists them until their first probe settles', async () => {
    let answer!: (mac: boolean) => void;
    vi.mocked(request).mockImplementation((path) => String(path).includes('/mac/')
      ? new Promise(resolve => { answer = mac => resolve({ connected: mac }); })
      : Promise.reject(new Error('unreachable')));
    const { result } = renderHook(() => useDevicePresence(portal));
    // Before any reply nothing reads as offline that the relay says is connected.
    expect(connected(result.current)).toEqual(['mac:true', 'nas:true', 'pi:false']);
    // A failed probe settles the NAS as offline; the Mac waits for its reply.
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(connected(result.current)).toEqual(['mac:true', 'nas:false', 'pi:false']);
    await act(async () => { answer(false); await vi.advanceTimersByTimeAsync(300); });
    expect(connected(result.current)).toEqual(['mac:false', 'nas:false', 'pi:false']);
  });
});
