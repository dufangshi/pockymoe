/** What a device grant allows, from the exact grant that will be signed. */
export interface GrantSummary {
  device: { name: string; id: string };
  recipient: { name: string; userId: string };
  safetyCode: string;
  scope: 'thread' | 'workspace' | 'device';
  workspaceScope: 'all' | 'selected';
  workspaceCount: number;
  threadAccess: 'read' | 'control';
  workspaceAccess: 'none' | 'read' | 'write';
  canCreateThreads: boolean;
  expiresAt: number | null;
  /** Fields that differ from the grant now on the device; absent for a new one. */
  changes?: GrantChange[];
}
export interface GrantChange {
  field: 'scope' | 'threadAccess' | 'workspaceAccess' | 'canCreateThreads' | 'expiresAt' | 'recipient';
  before: GrantSummary;
  after: GrantSummary;
}
export interface GrantReview {
  /** The complete signed content, shown under technical details. */
  description: string;
  recipientVerified: boolean;
  summary?: GrantSummary;
}
interface PendingReview extends GrantReview {
  id: number;
  resolve: (approved: boolean) => void;
}
let sequence = 0;
const queue: PendingReview[] = [];
const listeners = new Set<() => void>();
const notify = () => { for (const listener of listeners) listener(); };
export const currentGrantReview = () => queue[0];
export function subscribeGrantReview(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
// A missing confirmation UI must never turn into implicit authorization.
export function requestGrantReview(review: GrantReview): Promise<boolean> {
  if (!listeners.size) return Promise.resolve(false);
  return new Promise((resolve) => {
    queue.push({ ...structuredClone(review), id: ++sequence, resolve });
    notify();
  });
}
export function finishGrantReview(id: number, approved: boolean) {
  const pending = queue[0];
  if (!pending || pending.id !== id) return;
  queue.shift();
  pending.resolve(approved);
  notify();
}
