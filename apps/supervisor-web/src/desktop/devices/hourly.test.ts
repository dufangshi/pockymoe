import { describe, expect, it } from 'vitest';
import { aggregateHourly, runtimeHours, supportsHourly, type HourBucket, type StatsSummary } from './model';
import type { ThreadActivityResponseDto } from '@pockymoe/shared';

const buckets = (): HourBucket[] => Array.from({ length: 24 }, (_, hour) => ({ hour, turns: hour, failed: 0, tokens: hour * 100, costUsd: hour / 100 }));
const summary = (hourly = buckets()): StatsSummary => ({ range: 'today', generatedAt: '2026-11-01T06:30:00Z', hourly, daily: [], totals: { turns: 0, completed: 0, failed: 0, interrupted: 0, durationSamples: 0, avgDurationMs: null, p50DurationMs: null, inputTokens: 0, outputTokens: 0, cachedTokens: 0, costUsd: 0 }, byModel: [], byWorkspace: [], longestThreads: [], activeThreads: 0, runningTurns: 0 });
const activity = (since: string, hourlyMs: number[]): ThreadActivityResponseDto => ({ since, hours: hourlyMs.length, threads: [{ threadId: 't', turns: 400, activeMs: hourlyMs.reduce((n, value) => n + value, 0), totalMs: 10 ** 9, hourlyMs }] });
describe('hourly device charts', () => {
  it('requires 24 unique valid local-hour buckets and rejects missing or malformed optional data', () => {
    expect(supportsHourly(summary())).toBe(true);
    expect(supportsHourly({ ...summary(), range: '7d' })).toBe(false);
    expect(supportsHourly(undefined)).toBe(false);
    expect(supportsHourly({ ...summary(), hourly: undefined } as unknown as StatsSummary)).toBe(false);
    expect(supportsHourly(summary(buckets().slice(1)))).toBe(false);
    expect(supportsHourly(summary(buckets().map(row => ({ ...row, hour: 1 }))))).toBe(false);
    expect(supportsHourly(summary([null, ...buckets().slice(1)] as unknown as HourBucket[]))).toBe(false);
    expect(supportsHourly(summary(Array(24)))).toBe(false);
  });
  it('aggregates by hour identity and preserves exact independent usage, valid zeroes and unknown prices', () => {
    const other = buckets().reverse(); other.find(row => row.hour === 6)!.costUsd = null;
    const result = aggregateHourly([{ summary: summary() }, { summary: summary(other) }]);
    expect(result).toHaveLength(24);
    expect(result[0]).toEqual({ hour: 0, tokens: 0, costUsd: 0, pricePartial: false });
    expect(result[5]).toEqual({ hour: 5, tokens: 1000, costUsd: .1, pricePartial: false });
    expect(result[6]).toEqual({ hour: 6, tokens: 1200, costUsd: .06, pricePartial: true });
    expect(result[7]?.tokens).toBe(1400); // Not a cumulative sum.
  });
  it('does not treat a device missing hourly data as zero usage', () => {
    const legacy = summary(); delete legacy.hourly;
    expect(aggregateHourly([{ summary: summary() }, { summary: legacy }]).every(row => row.tokens === null && row.costUsd === null)).toBe(true);
    expect(aggregateHourly([]).every(row => row.tokens === null)).toBe(true);
  });
  it('folds repeated local DST hours and conserves sampled runtime without inventing turns', () => {
    const result = runtimeHours(activity('2026-11-01T04:30:00Z', [3_600_000, 3_600_000]), 'America/Toronto', new Date('2026-11-01T06:30:00Z'));
    expect(result[0]).toBe(1_800_000); expect(result[1]).toBe(5_400_000);
    expect(result[2]).toBeNull(); expect(result.reduce<number>((n, value) => n + (value ?? 0), 0)).toBe(7_200_000);
  });
  it('handles fractional timezone offsets and clips yesterday from the today view', () => {
    const nepal = runtimeHours(activity('2026-11-01T00:00:00Z', [3_600_000]), 'Asia/Kathmandu', new Date('2026-11-01T01:00:00Z'));
    expect(nepal[5]).toBe(900_000); expect(nepal[6]).toBe(2_700_000);
    const utc = runtimeHours(activity('2026-10-31T23:30:00Z', [3_600_000, 3_600_000]), 'UTC', new Date('2026-11-01T01:30:00Z'));
    expect(utc[0]).toBe(3_600_000); expect(utc[1]).toBe(1_800_000); expect(utc[23]).toBeNull();
  });
  it('leaves absent/stale/malformed samples unknown and keeps covered idle periods at zero', () => {
    expect(runtimeHours(undefined, 'UTC').every(value => value === null)).toBe(true);
    expect(runtimeHours(activity('2026-10-30T00:00:00Z', [1]), 'UTC', new Date('2026-11-01T12:00:00Z')).every(value => value === null)).toBe(true);
    const missing = activity('2026-11-01T00:00:00Z', [1]); delete missing.threads[0]!.hourlyMs;
    expect(runtimeHours(missing, 'UTC', new Date('2026-11-01T01:00:00Z')).every(value => value === null)).toBe(true);
    expect(runtimeHours(activity('2026-11-01T00:00:00Z', [0, 0]), 'UTC', new Date('2026-11-01T02:00:00Z')).slice(0, 3)).toEqual([0, 0, null]);
  });
});
