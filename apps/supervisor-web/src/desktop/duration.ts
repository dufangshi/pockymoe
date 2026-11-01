/** Compact running clock: m:ss below an hour, h:mm:ss thereafter. The bridge
 * continues to exchange numeric durationMs; presentation never changes units. */
export function formatRunTime(durationMs: number | null | undefined): string {
  if (durationMs == null || !Number.isFinite(durationMs)) return '—';
  const seconds = Math.max(0, Math.floor(durationMs / 1000));
  const hours = Math.floor(seconds / 3600), minutes = Math.floor(seconds % 3600 / 60);
  const tail = `${String(minutes).padStart(hours ? 2 : 1, '0')}:${String(seconds % 60).padStart(2, '0')}`;
  return hours ? `${hours}:${tail}` : tail;
}
