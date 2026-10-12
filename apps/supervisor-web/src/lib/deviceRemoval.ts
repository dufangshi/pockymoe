import { deleteRelayDevice } from './api';
import { revokeDevice } from './deviceEnrollment';

/** Revoke account trust before invalidating the device's relay registration. */
export async function removeDeviceFromAccount(deviceId: string) {
  await revokeDevice(deviceId);
  await deleteRelayDevice(deviceId);
}
