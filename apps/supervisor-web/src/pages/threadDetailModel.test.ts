import { expect, it } from 'vitest';
import type { ContextInspectionSnapshotDto } from '../../../../packages/shared/src/index';
import { mergeThreadContextUsage } from './threadDetailModel';

const base = { availability: 'available' as const, remainingPercent: 90, tokensInContextWindow: 10, modelContextWindow: 100, updatedAt: '2026-11-01T00:00:00Z' };
const details = { source: 'codexTrace', usedTokens: 8 } as ContextInspectionSnapshotDto;

it('keeps the last analysis through a plain usage update, and takes a newer one', () => {
  expect(mergeThreadContextUsage({ ...base, details }, base).details).toBe(details);
  expect(mergeThreadContextUsage(undefined, base)).toBe(base);
  const newer = { ...details, usedTokens: 9 };
  expect(mergeThreadContextUsage({ ...base, details }, { ...base, details: newer }).details).toBe(newer);
});
