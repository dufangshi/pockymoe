import { appUrl } from './webBase';
import { translate } from '@pockymoe/thread-ui/i18n';
import type { PublicTranscriptSnapshot } from '@pockymoe/thread-ui';

export interface PublicLinkEnvelope {
  version: 2;
  iv: string;
  ciphertext: string;
  stale?: boolean;
}
export class PublicLinkKeyError extends Error {
  constructor(public readonly reason: 'missing' | 'wrong') {
    super(reason);
  }
}
const aad = new TextEncoder().encode('pockymoe-public-link-v1');
const maxSnapshotBytes = 16 * 1024 * 1024;
const storagePrefix = 'pockymoe-public-link-key:';
const keys = new Map<string, string>();

// Unpadded base64url is shared with the Rust publisher. AES-GCM's tag is
// appended to ciphertext by WebCrypto. The key is only in the URL fragment
// and this browser's local storage, never in a relay request or snapshot.
function encode(bytes: Uint8Array): string {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 8192)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
function decode(encoded: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]*$/.test(encoded)) throw new Error('Invalid encoding');
  const bytes = Uint8Array.from(atob(encoded.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0));
  if (encode(bytes) !== encoded) throw new Error('Invalid encoding');
  return bytes;
}
async function importKey(key: string, usage: KeyUsage) {
  const bytes = decode(key);
  if (bytes.length !== 32) throw new Error('Invalid key');
  return crypto.subtle.importKey('raw', bytes, 'AES-GCM', false, [usage]);
}
export function createPublicLinkKey(): string {
  return encode(crypto.getRandomValues(new Uint8Array(32)));
}
export async function encryptPublicSnapshot(snapshot: PublicTranscriptSnapshot, key: string): Promise<PublicLinkEnvelope> {
  const plaintext = new TextEncoder().encode(JSON.stringify(snapshot));
  if (plaintext.length > maxSnapshotBytes) throw new Error(translate("sharing.theTranscriptIsTooLargeToShare"));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: aad }, await importKey(key, 'encrypt'), plaintext);
  return { version: 2, iv: encode(iv), ciphertext: encode(new Uint8Array(ciphertext)) };
}
export async function decryptPublicSnapshot(value: PublicTranscriptSnapshot | PublicLinkEnvelope, fragment: string): Promise<PublicTranscriptSnapshot> {
  const version = (value as { version?: number }).version;
  if (version === undefined || version === 1) return value as PublicTranscriptSnapshot;
  const envelope = value as PublicLinkEnvelope;
  const key = new URLSearchParams(fragment.replace(/^#/, '')).get('k');
  if (!key) throw new PublicLinkKeyError('missing');
  try {
    if (version !== 2 || typeof envelope.iv !== 'string' || typeof envelope.ciphertext !== 'string'
      || envelope.iv.length !== 16 || envelope.ciphertext.length > Math.ceil((maxSnapshotBytes + 16) / 3) * 4)
      throw new Error('Invalid envelope');
    const iv = decode(envelope.iv);
    const ciphertext = decode(envelope.ciphertext);
    if (iv.length !== 12 || ciphertext.length < 16 || ciphertext.length > maxSnapshotBytes + 16) throw new Error('Invalid envelope');
    const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv, additionalData: aad }, await importKey(key, 'decrypt'), ciphertext);
    const snapshot = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(plaintext)) as PublicTranscriptSnapshot;
    if (!snapshot || typeof snapshot.title !== 'string' || !Array.isArray(snapshot.turns)) throw new Error('Invalid snapshot');
    // Staleness belongs to the relay cache, outside the authenticated snapshot.
    return { ...snapshot, stale: envelope.stale === true };
  } catch {
    throw new PublicLinkKeyError('wrong');
  }
}
export function rememberPublicLinkKey(id: string, key: string): void {
  keys.set(id, key);
  try { localStorage.setItem(storagePrefix + id, key); } catch { /* Memory retains the freshly created URL. */ }
}
export function forgetPublicLinkKey(id: string): void {
  keys.delete(id);
  try { localStorage.removeItem(storagePrefix + id); } catch { /* Storage may be unavailable. */ }
}
export function publicLinkUrl(id: string, version?: number): string | null {
  const base = `${location.origin}${appUrl(`s/${encodeURIComponent(id)}`)}`;
  if (version !== 2) return base;
  let key = keys.get(id);
  try { key ??= localStorage.getItem(storagePrefix + id) ?? undefined; } catch { /* Storage may be unavailable. */ }
  return key ? `${base}#k=${key}` : null;
}
