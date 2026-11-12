import { currentAccount } from './accountVault';
import { isAccountUnlocked } from './accountVaultState';
import { request } from './api';
import {
  completeDeviceEnrollment,
  ENROLLMENT_CHANGED_EVENT,
  listPendingEnrollments,
} from './deviceEnrollment';

const INTERVAL_MS = 5000;

/**
 * Finishes a new device's protection as soon as it connects, while the account
 * is still unlocked from creating its setup command. It never asks for the
 * password (the loop that did interrupted work, see docs/account-key-trust.md):
 * once the unlock window has closed, the device's card, its row and its blocked
 * page offer "Finish protection" instead.
 */
export function startQuietEnrollment(): () => void {
  let stopped = false;
  let running = false;
  const tick = async () => {
    if (stopped || running || document.visibilityState !== 'visible' || !isAccountUnlocked()) return;
    running = true;
    try {
      const account = await currentAccount();
      if (!account) return;
      // Remote protection has its own flow and its own prompts.
      const pending = (await listPendingEnrollments(account)).filter((enrollment) => !enrollment.remoteProtection);
      for (const enrollment of pending) {
        if (stopped || !isAccountUnlocked()) break;
        try {
          const presence = await request<{ connected?: boolean }>(
            `relay/devices/${encodeURIComponent(enrollment.deviceId)}/presence`, { cache: 'no-store' });
          if (!presence.connected) continue;
          // A finished enrollment leaves the pending list, which announces the change.
          await completeDeviceEnrollment(enrollment, true, undefined, { quiet: true });
        } catch {
          // Still starting, or it needs the reader (a changed identity): the
          // device's own controls report and fix that.
        }
      }
    } finally {
      running = false;
    }
  };
  const timer = window.setInterval(() => { void tick(); }, INTERVAL_MS);
  const now = () => { void tick(); };
  window.addEventListener(ENROLLMENT_CHANGED_EVENT, now);
  return () => {
    stopped = true;
    window.clearInterval(timer);
    window.removeEventListener(ENROLLMENT_CHANGED_EVENT, now);
  };
}
