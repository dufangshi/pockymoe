import { beforeEach, describe, expect, it, vi } from 'vitest';
import { deleteRelayDevice } from './api';
import { revokeDevice } from './deviceEnrollment';
import { removeDeviceFromAccount } from './deviceRemoval';
vi.mock('./api', () => ({ deleteRelayDevice: vi.fn() }));
vi.mock('./deviceEnrollment', () => ({ revokeDevice: vi.fn() }));
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(revokeDevice).mockResolvedValue(undefined);
  vi.mocked(deleteRelayDevice).mockResolvedValue({ id: 'device' });
});
describe('remove device from account', () => {
  it('waits for revocation and vault removal before deleting the relay registration', async () => {
    let finishTrustRemoval!: () => void;
    vi.mocked(revokeDevice).mockReturnValue(
      new Promise<void>((resolve) => {
        finishTrustRemoval = resolve;
      }),
    );
    const removal = removeDeviceFromAccount('device');
    expect(revokeDevice).toHaveBeenCalledWith('device');
    expect(deleteRelayDevice).not.toHaveBeenCalled();
    finishTrustRemoval();
    await removal;
    expect(deleteRelayDevice).toHaveBeenCalledExactlyOnceWith('device');
  });
  it('keeps the registration when posting revocation or updating boundDevices fails', async () => {
    vi.mocked(revokeDevice).mockRejectedValue(new Error('vault_conflict'));
    await expect(removeDeviceFromAccount('device')).rejects.toThrow(
      'vault_conflict',
    );
    expect(deleteRelayDevice).not.toHaveBeenCalled();
  });
  it('surfaces a registration deletion failure after trust has been removed', async () => {
    vi.mocked(deleteRelayDevice).mockRejectedValue(
      new Error('relay_unavailable'),
    );
    await expect(removeDeviceFromAccount('device')).rejects.toThrow(
      'relay_unavailable',
    );
    expect(revokeDevice).toHaveBeenCalledExactlyOnceWith('device');
    expect(deleteRelayDevice).toHaveBeenCalledExactlyOnceWith('device');
  });
});
