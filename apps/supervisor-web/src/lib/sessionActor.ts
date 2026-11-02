// Synchronous identity boundary for decrypted, in-memory UI data. It must run
// before an account switch can render, including switches without a page reload.
let actor: string | null | undefined;
let revision = 0;
const listeners = new Set<() => void>();
export const sessionRevision = () => revision;
export const subscribeSessionActor = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
function changeActor(next: string | null) {
  if (actor === next) return false;
  actor = next; revision++;
  for (const listener of listeners) listener();
  return true;
}
export const currentSessionActor = () => actor;
const actorStorageKey = 'pockymoe-session-actor';
export function observeSessionActor(next: string | null) {
  if (!changeActor(next)) return;
  try { window.localStorage.setItem(actorStorageKey, JSON.stringify(next)); } catch { /* Memory invalidation still works without storage. */ }
}
if (typeof window !== 'undefined') window.addEventListener('storage', event => {
  if (event.key !== actorStorageKey) return;
  try { const value: unknown = JSON.parse(event.newValue ?? 'null'); if (value === null || typeof value === 'string') changeActor(value); } catch { /* Ignore unrelated invalid storage. */ }
});

if (typeof window !== 'undefined') window.addEventListener('pockymoe-account-logout', () => observeSessionActor(null));
