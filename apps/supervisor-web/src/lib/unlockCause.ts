import type { UnlockReason } from './accountVault';

/**
 * Why the unlock prompt opened. The prompt shows it, and copies it for support, so a
 * report says which device refused which request with which code.
 */
export interface UnlockCause {
  kind: 'missing-browser-key' | 'device-refused' | 'browser-removed' | 'account-missing' | 'action';
  /** For `action`: the operation that needs the account key. */
  action?: UnlockReason | undefined;
  deviceId?: string | undefined;
  method?: string | undefined;
  path?: string | undefined;
  code?: string | undefined;
  /** Where in the page it started, when no request was involved. */
  source?: string | undefined;
  at: number;
}

export function unlockCause(kind: UnlockCause['kind'], fields: Omit<UnlockCause, 'kind' | 'at'> = {}): UnlockCause {
  return { kind, ...fields, at: Date.now() };
}
