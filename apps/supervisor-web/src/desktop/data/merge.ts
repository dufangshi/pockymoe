function equal(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object' || Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>, right = b as Record<string, unknown>;
  const keys = Object.keys(left).filter(key => left[key] !== undefined);
  return keys.length === Object.keys(right).filter(key => right[key] !== undefined).length && keys.every(key => equal(left[key], right[key]));
}
/** Keep unchanged entity references and keep the array when order is unchanged. */
export function mergeRows<T extends object>(previous: readonly T[], incoming: readonly T[], key: (row: T) => string): readonly T[] {
  const byId = new Map(previous.map(row => [key(row), row]));
  const next = incoming.map(row => { const old = byId.get(key(row)); return old && equal(old, row) ? old : row; });
  return previous.length === next.length && next.every((row, i) => row === previous[i]) ? previous : next;
}
