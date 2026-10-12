export interface GrantReview {
  description: string;
  recipientVerified: boolean;
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
