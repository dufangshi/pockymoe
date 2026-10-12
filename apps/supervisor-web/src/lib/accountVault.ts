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

export { inputs as signingInput } from './accountCrypto';
export { currentAccount, currentBrowser, boundDevices, requireUnlock,
  verifyDeviceCertificate, safetyCode } from './accountVaultState';
