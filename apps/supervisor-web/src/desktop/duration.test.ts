import { expect, it } from 'vitest';
import { formatRunTime } from './duration';
it.each([
  [null, '—'], [NaN, '—'], [-1000, '0:00'], [59_999, '0:59'],
  [3_599_000, '59:59'], [3_600_000, '1:00:00'], [5_170_000, '1:26:10'], [90_061_000, '25:01:01'],
] as const)('formats %s milliseconds as %s without unbounded minutes', (ms, result) => {
  expect(formatRunTime(ms)).toBe(result);
});
