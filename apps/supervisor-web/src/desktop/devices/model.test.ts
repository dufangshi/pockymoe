import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { aggregate, age, calendarDays, compact, deviceColor, deviceCounts, duration, isUnsupported, lifetimeActivity, money, newerVersion, partialSummary, supportsStats, type StatsSummary } from './model';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import type { DesktopIndex, DesktopThread } from '../data/types';

beforeEach(() => setLocale('zh-CN', false));
afterEach(() => setLocale('en', false));
const now = new Date('2026-11-01T18:00:00Z');
const thread = (extra: Partial<DesktopThread>): DesktopThread => ({ providerSessionId: null, source: 'supervisor', model: 'gpt-6.1', reasoningEffort: null, collaborationMode: 'default', approvalMode: 'yolo', summaryText: null, lastError: null, isLoaded: false, lastTurnId: null, key: 'd:t', deviceId: 'd', id: 't', workspaceId: 'w', title: 'Thread', provider: 'codex', status: 'idle', isPinned: false, createdAt: now.toISOString(), updatedAt: now.toISOString(), lastTurnStartedAt: '2026-11-01T17:00:00Z', lastTurnCompletedAt: '2026-11-01T17:01:00Z', activeTurnId: null, parentThreadId: null, rootThreadId: null, agentName: null, closedAt: null, lastViewedAt: null, requests: [], resultSummary: null, ...extra });
const index = (threads: DesktopThread[] = []): DesktopIndex => ({ accountId: 'a', devices: [], workspaces: [{ key: 'd:w', id: 'w', hostId: 'h', deviceId: 'd', absPath: '/fixture', label: 'Code', createdAt: now.toISOString(), lastOpenedAt: null, isFavorite: false }], threads, activity: {}, extras: {}, pulseKeys: [], hydrated: true, refreshing: false, error: null });
const stats = (extra: Partial<StatsSummary['totals']> = {}): StatsSummary => ({ range: 'today', generatedAt: now.toISOString(), totals: { turns: 5, completed: 3, failed: 1, interrupted: 0, avgDurationMs: 1000, p50DurationMs: 1000, inputTokens: 100, outputTokens: 30, cachedTokens: 60, costUsd: .1, durationSamples: 4, ...extra }, daily: [{ date: '2026-11-01', turns: 5, failed: 1, tokens: 130, costUsd: .1 }], byModel: [{ model: 'm', turns: 5, tokens: 130, costUsd: .1 }], byWorkspace: [{ workspaceId: 'w', label: 'Code', turns: 5, tokens: 130, costUsd: .1, lastActivityAt: now.toISOString() }], longestThreads: [{ threadId: 't', title: 'Thread', workspaceId: 'w', durationMs: 1000 }], activeThreads: 1, runningTurns: 1 });
describe('desktop device statistics', () => {
  it('uses calendar days across daylight saving and timezone boundaries', () => {
    const days = calendarDays('7d', 'America/Toronto', new Date('2026-11-02T02:00:00Z'));
    expect(days).toHaveLength(7); expect(days[0]).toBe('2026-10-26'); expect(days[6]).toBe('2026-11-01');
    expect(calendarDays('today', 'Asia/Tokyo', now)).toEqual(['2026-11-02']);
  });
  it('counts only known latest turns in the fallback and does not invent tokens, models, costs or durations', () => {
    const summary = partialSummary(index([thread({}), thread({ id: 'bad', lastTurnStartedAt: 'bad' }), thread({ id: 'future', lastTurnStartedAt: '2027-01-01T00:00:00Z' }), thread({ id: 'other', deviceId: 'other' }), thread({ id: 'old', lastTurnStartedAt: '2026-10-20T00:00:00Z' })]), 'd', 'today', 'UTC', now);
    expect(summary.totals.turns).toBe(1); expect(summary.totals.completed).toBe(1);
    expect(summary.totals.costUsd).toBeNull(); expect(summary.totals.avgDurationMs).toBeNull();
    expect(summary.totals.inputTokens).toBe(0); expect(summary.byModel).toEqual([]); expect(summary.byWorkspace[0]?.turns).toBe(1);
  });
  it('does not count an old completion as completion of a currently running turn', () => {
    const summary = partialSummary(index([thread({ status: 'running', lastTurnStartedAt: '2026-11-01T17:30:00Z', activeTurnId: 'new' })]), 'd', 'today', 'UTC', now);
    expect(summary.totals.turns).toBe(1); expect(summary.totals.completed).toBe(0); expect(summary.runningTurns).toBe(1);
  });
  it('aggregates valid duration samples, terminal success rates and tokens without adding cache reads twice', () => {
    const result = aggregate([{ deviceId: 'a', summary: stats() }, { deviceId: 'b', summary: stats({ avgDurationMs: 5000, durationSamples: 1 }) }]);
    expect(result.avgDurationMs).toBe(1800); expect(result.successRate).toBe(.75);
    expect(result.tokens).toBe(260); expect(result.cachedTokens).toBe(120); expect(result.costUsd).toBe(.2);
    expect(result.daily[0]?.turns).toBe(10); expect(result.models[0]?.turns).toBe(10);
    expect(result.workspaces).toHaveLength(2); // IDs can collide across devices.
  });
  it('preserves unknown costs and refuses an inexact cross-device mean when sample counts are absent', () => {
    const result = aggregate([{ deviceId: 'a', summary: stats() }, { deviceId: 'b', summary: stats({ durationSamples: undefined, costUsd: null }) }]);
    expect(result.avgDurationMs).toBeNull(); expect(result.costUsd).toBeNull();
    expect(aggregate([{ deviceId: 'a', summary: stats({ durationSamples: undefined }) }]).avgDurationMs).toBe(1000);
  });
  it('detects capabilities strictly and degrades only unsupported HTTP statuses', () => {
    expect(supportsStats({})).toBe(false); expect(supportsStats({ capabilities: { statsSummary: false } })).toBe(false);
    expect(supportsStats({ capabilities: { statsSummary: true } })).toBe(true);
    expect(isUnsupported({ statusCode: 404 })).toBe(true); expect(isUnsupported({ statusCode: 500 })).toBe(false); expect(isUnsupported(null)).toBe(false);
  });
  it('formats unknown and zero values distinctly and keeps colors stable', () => {
    expect(money(null)).toBe('—'); expect(money(0)).toBe('$0.00'); expect(compact(1200000)).toBe('120万');
    expect(duration(72000)).toBe('1 分 12 秒'); expect(duration(null)).toBe('—');
    expect(age('2026-11-01T15:00:00Z', +now)).toBe('3 小时前'); expect(age('bad', +now)).toBe('时间未知');
    expect(deviceColor('abc')).toBe(deviceColor('abc'));
  });
  it('shows an update only for a newer stable semantic version', () => {
    expect(newerVersion('0.12.59', '0.12.60')).toBe(true);
    expect(newerVersion('0.12.9', '0.12.10')).toBe(true);
    expect(newerVersion('0.13.0', '0.12.60')).toBe(false);
    expect(newerVersion('development', '0.12.60')).toBe(false);
  });
  it('computes counts from the device-scoped index including pending input', () => {
    expect(deviceCounts(index([thread({ status: 'recovering' }), thread({ id: 'input', requests: [{ id: 'r', kind: 'input', summary: 'Choose', createdAt: now.toISOString() }] }), thread({ deviceId: 'other' })]), 'd')).toEqual({ running: 1, attention: 1, workspaces: 1, threads: 2 });
  });
  it('counts conversations once when their agent threads also run', () => {
    expect(deviceCounts(index([thread({ status: 'running' }), thread({ id: 'child', rootThreadId: 't', parentThreadId: 't', status: 'running' })]), 'd')).toMatchObject({ running: 1, threads: 1 });
  });
  it('keeps unread failures separate from the waiting-for-input count', () => {
    expect(deviceCounts(index([thread({ status: 'failed' })]), 'd').attention).toBe(0);
  });
  it('keeps thread-activity lifetime usage separate from calendar ranges and preserves unknown prices', () => {
    const all = lifetimeActivity(index([thread({})]), 'd', { since: '2026-10-25T00:00:00Z', hours: 168, threads: [{ threadId: 't', turns: 100, activeMs: 10, totalMs: 1000, tokenUsage: { totalTokens: 200, inputTokens: 150, cachedInputTokens: 50, outputTokens: 50, reasoningOutputTokens: 0 }, pricePartial: true, priceEstimate: { currency: 'USD', inputUsd: 1, cachedInputUsd: .1, outputUsd: .5, totalUsd: 1.6 } }] });
    expect(all.turns).toBe(100); expect(all.tokens).toBe(200); expect(all.costUsd).toBeNull(); expect(all.workspaces[0]?.turns).toBe(100);
  });
});
