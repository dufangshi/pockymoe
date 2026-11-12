import { ApiError, deleteRelayDevice } from './api';
import { revokeDevice } from './deviceEnrollment';

/** Revoke account trust before invalidating the device's relay registration. */
export async function removeDeviceFromAccount(deviceId: string) {
  await revokeDevice(deviceId);
  try {
    await deleteRelayDevice(deviceId);
  } catch (error) {
    // Removed already (another browser, or an earlier attempt that lost its
    // answer): the account no longer lists it, which is what was asked.
    if (!(error instanceof ApiError) || error.statusCode !== 404) throw error;
  }
}
