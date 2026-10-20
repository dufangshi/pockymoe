import { currentAccount } from './accountVault';
import {
  completeDeviceEnrollment,
  ENROLLMENT_CHANGED_EVENT,
  listPendingEnrollments,
} from './deviceEnrollment';
import { runRemoteProtection } from './remoteDeviceProtection';

/**
 * Finish a device's pending protection now, when the reader asks for it. There is
 * no background loop doing this: a loop that needs the account key interrupted
 * work with password prompts. This may ask for the password once, on purpose.
 * Returns whether every pending enrollment for the device completed.
 */
export async function finishPendingProtection(deviceId: string) {
  const account = await currentAccount();
  if (!account) return false;
  const pending = (await listPendingEnrollments(account)).filter((enrollment) => enrollment.deviceId === deviceId);
  let completed = pending.length > 0;
  for (const enrollment of pending) {
    const done = enrollment.remoteProtection
      ? await runRemoteProtection(enrollment, true)
      : await completeDeviceEnrollment(enrollment, true);
    completed &&= Boolean(done);
  }
  window.dispatchEvent(new Event(ENROLLMENT_CHANGED_EVENT));
  return completed;
}
