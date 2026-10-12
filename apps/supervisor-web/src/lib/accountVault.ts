// Account key trust (protocol v2): the browser's account key, vault and browser
// identity. This file is the shared contract between Web modules; `trust-web`
// implements it, other modules only import it (and mock it in tests).

export interface AccountKeyInfo {
  userId: string;
  accountKeyId: string;
  /** B64 SPKI of the account's ECDSA P-256 public key. */
  accountPublicKey: string;
}

export interface BrowserCertificate {
  version: 1;
  accountKeyId: string;
  browserId: string;
  publicKey: string;
  createdAt: number;
  signature: string;
}

export interface DeviceCertificate {
  version: 1;
  accountKeyId: string;
  deviceId: string;
  identityKey: string;
  createdAt: number;
  signature: string;
}

export interface Revocation {
  version: 1;
  accountKeyId: string;
  kind: 'browser' | 'device';
  subjectId: string;
  revokedAt: number;
  signature: string;
}

export interface BrowserIdentity extends AccountKeyInfo {
  browserId: string;
  certificate: BrowserCertificate;
  /** Non-extractable ECDSA key used for request signatures. */
  privateKey: CryptoKey;
}

/** Actions that need the account key, shown in the password prompt. */
export type UnlockReason =
  | 'connect'
  | 'bind-device'
  | 'certify-device'
  | 'revoke'
  | 'change-password'
  | 'recovery-code';

/** The account key while an unlock window is open (at most 10 minutes). */
export interface UnlockedAccount extends AccountKeyInfo {
  /** Signs a protocol signing input with the account key (B64 raw r||s). */
  sign(input: string): Promise<string>;
  /** Read-modify-write of the vault's bound device list (next revision). */
  updateBoundDevices(update: (deviceIds: string[]) => string[]): Promise<void>;
}

const pending = (name: string): never => {
  throw new Error(`accountVault.${name} is not implemented yet`);
};

/** Cached account key of the signed-in user, or null if this browser has none. */
export async function currentAccount(): Promise<AccountKeyInfo | null> {
  return pending('currentAccount');
}

/** This browser's registered identity for the signed-in user, or null. */
export async function currentBrowser(): Promise<BrowserIdentity | null> {
  return pending('currentBrowser');
}

/** Device ids the last opened vault lists as bound (they must present certificates). */
export async function boundDevices(): Promise<string[]> {
  return pending('boundDevices');
}

/**
 * Returns the open unlock window, or asks the user for the password (or recovery
 * code) through the shared prompt. Rejects if the user cancels.
 */
export async function requireUnlock(_reason: UnlockReason): Promise<UnlockedAccount> {
  return pending('requireUnlock');
}

/** Section 3 signing inputs. */
export const signingInput = {
  browser: (c: Omit<BrowserCertificate, 'signature' | 'version'>) =>
    `pockymoe-acct-browser-v1\n${c.accountKeyId}\n${c.browserId}\n${c.publicKey}\n${c.createdAt}`,
  device: (c: Omit<DeviceCertificate, 'signature' | 'version'>) =>
    `pockymoe-acct-device-v1\n${c.accountKeyId}\n${c.deviceId}\n${c.identityKey}\n${c.createdAt}`,
  revocation: (r: Omit<Revocation, 'signature' | 'version'>) =>
    `pockymoe-acct-revoke-v1\n${r.accountKeyId}\n${r.kind}\n${r.subjectId}\n${r.revokedAt}`,
};

/** Verifies a device certificate for this device id and identity key. */
export async function verifyDeviceCertificate(
  _certificate: DeviceCertificate,
  _account: AccountKeyInfo,
  _deviceId: string,
  _identityKey: string,
): Promise<boolean> {
  return pending('verifyDeviceCertificate');
}

/** Crockford base32 safety code of an account key id, grouped by 4. */
export async function safetyCode(_accountKeyId: string): Promise<string> {
  return pending('safetyCode');
}
