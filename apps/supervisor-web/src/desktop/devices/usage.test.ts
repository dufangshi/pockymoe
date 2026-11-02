import { afterEach, expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import type { DesktopIndex } from '../data/types';
import { accountUsage, costLabel, usageColor } from './usage';
import { money, partialSummary, type StatsSummary } from './model';
const index = { accountId: 'a', devices: [], workspaces: [], threads: [], activity: {}, extras: {}, pulseKeys: [], hydrated: true, refreshing: false, error: null } as DesktopIndex;
const summary = (): StatsSummary => ({ ...partialSummary(index, 'a', 'today', 'UTC'), totals: { turns: 5, completed: 4, failed: 1, interrupted: 0, inputTokens: 8000, outputTokens: 2000, cachedTokens: 3000, avgDurationMs: 100, p50DurationMs: 100, costUsd: 6, unpricedTokens: 2000 }, byModel: [{ model: 'Fast', turns: 4, tokens: 8000, costUsd: 1 }, { model: 'Premium', turns: 1, tokens: 2000, costUsd: 5, unpricedTokens: 2000 }], byWorkspace: [{ workspaceId: 'same', label: 'Code', turns: 5, tokens: 10000, costUsd: 6, lastActivityAt: '2026-11-01' }] });
afterEach(() => setLocale('en', false));
it('sorts models, devices and workspaces by the chosen metric while preserving model colors', () => {
  const first = summary(), second = summary(); second.totals.inputTokens = 18000; second.totals.costUsd = 1;
  const sources = [{ deviceId: 'a', name: 'A', summary: first }, { deviceId: 'b', name: 'B', summary: second }];
  const tokens = accountUsage(sources, index, 'today', 'tokens'), cost = accountUsage(sources, index, 'today', 'cost');
  expect(tokens.models.map(row => row.id)).toEqual(['Fast', 'Premium']); expect(cost.models.map(row => row.id)).toEqual(['Premium', 'Fast']);
  expect(tokens.total.tokens).toBe(30000);
  expect(tokens.devices.map(row => row.id)).toEqual(['b', 'a']); expect(cost.devices.map(row => row.id)).toEqual(['a', 'b']);
  expect(tokens.devices.flatMap(row => row.workspaces)).toHaveLength(2);
  expect(usageColor(tokens.models[0]!.id)).toBe(usageColor(cost.models[1]!.id));
  expect(usageColor('GPT-6.1')).not.toBe(usageColor('Claude Sonnet 4.6'));
});
it('keeps known cost when some models are unpriced, distinguishing unknown from free usage', () => {
  const stats = summary(); stats.byModel.push({ model: 'Fast', turns: 1, tokens: 2000, costUsd: null });
  const usage = accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'cost');
  const fast = usage.models.find(row => row.id === 'Fast')!;
  expect(fast.costUsd).toBe(1); expect(fast.unpricedTokens).toBe(2000); expect(costLabel(fast)).toBe('≥ $1.00');
  expect(costLabel({ costUsd: null })).toBe('—'); expect(costLabel({ costUsd: 0 })).toBe('$0.00');
  expect(costLabel(usage.total)).toBe('≥ $6.00');
});
it('does not invent per-model or range usage from lifetime activity and includes threads missing from the index', () => {
  const source = { deviceId: 'old', name: 'Old', activity: { since: '2026-11-01', hours: 24, threads: [{ threadId: 'not-indexed', turns: 50, activeMs: 100, totalMs: 200, tokenUsage: { inputTokens: 2000, outputTokens: 1000, totalTokens: 3000, cachedInputTokens: 0, reasoningOutputTokens: 0 }, priceEstimate: { currency: 'USD' as const, inputUsd: 1, outputUsd: 1, cachedInputUsd: 0, cacheWriteInputUsd: 0, totalUsd: 2 }, pricePartial: true }] } };
  const today = accountUsage([source], index, 'today', 'tokens'), all = accountUsage([source], index, 'all', 'tokens');
  expect(today.total.tokens).toBe(0); expect(today.partial).toBe(true);
  expect(all.total.tokens).toBe(3000); expect(all.models[0]!.inputTokens).toBe(2000); expect(all.models[0]!.outputTokens).toBe(1000); expect(all.total.turns).toBe(50); expect(all.models.map(row => row.id)).toEqual(['unknown']); expect(costLabel(all.total)).toBe('≥ $2.00');
});
it('excludes unsupported origin detail rather than attributing all turns to the selected origin', () => {
  const stats = summary();
  expect(accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'tokens', 'guest').total.tokens).toBe(0);
  stats.origin = 'guest';
  stats.totals.turns = 1; stats.totals.inputTokens = 1000; stats.totals.outputTokens = 1000;
  stats.byModel = [{ model: 'Fast', turns: 1, tokens: 2000, costUsd: 2 }];
  const scoped = accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'tokens', 'guest');
  expect(scoped.total.turns).toBe(1); expect(scoped.models[0]!.tokens).toBe(2000);
});
it('uses the locale compact notation for large prices and never formats an unknown estimate', () => {
  setLocale('en', false); expect(money(3070)).toBe('$3.07K'); expect(money(3070000)).toBe('$3.07M');
  setLocale('zh-CN', false); expect(money(307000000)).toBe('$3.07亿'); expect(money(null)).toBe('—');
});
it('distinguishes missing legacy token evidence from a recorded zero even when costs exist', () => {
  const source = { deviceId: 'old', name: 'Legacy', activity: { since: '2026-11-01', hours: 24, threads: [{ threadId: 't', turns: 17, activeMs: 0, totalMs: 100, priceEstimate: { currency: 'USD' as const, totalUsd: 207.58, inputUsd: 100, outputUsd: 107.58, cachedInputUsd: 0, cacheWriteInputUsd: 0 } }] } };
  const data = accountUsage([source], index, 'all', 'tokens');
  expect(data.total.tokenUnknown).toBe(true); expect(data.models[0]!.tokenUnknown).toBe(true);
  expect(data.total.costUsd).toBe(207.58); expect(data.devices[0]!.workspaces[0]!.tokenUnknown).toBe(true);
});
it('uses execution totals instead of summing overlapping per-model turn counts', () => {
  const stats = summary(); stats.totals.turns = 2; stats.byModel.forEach(model => { model.turns = 2; });
  const data = accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'tokens');
  expect(data.models.reduce((sum, row) => sum + row.turns, 0)).toBe(4); expect(data.total.turns).toBe(2);
});
it('does not treat an empty device zero as evidence of known pricing for unpriced usage', () => {
  const unpriced = summary(); unpriced.totals.costUsd = null; unpriced.totals.unpricedTokens = 10000; unpriced.byModel.forEach(row => { row.costUsd = null; });
  const empty = summary(); empty.totals.turns = 0; empty.totals.inputTokens = 0; empty.totals.outputTokens = 0; empty.totals.costUsd = 0; empty.totals.unpricedTokens = 0; empty.byModel = []; empty.byWorkspace = [];
  const sources = [{ deviceId: 'a', name: 'A', summary: unpriced }, { deviceId: 'empty', name: 'Empty', summary: empty }];
  expect(costLabel(accountUsage(sources, index, 'today', 'cost').total)).toBe('—');
  expect(costLabel(accountUsage([sources[1]!], index, 'today', 'cost').total)).toBe('$0.00');
});
it('recovers known model subtotals from legacy null aggregate pricing without losing unknown coverage', () => {
  const stats = summary(); stats.totals.costUsd = null; delete stats.totals.unpricedTokens;
  stats.byModel.push({ model: 'Unpriced', turns: 1, tokens: 300, costUsd: null });
  const data = accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'cost');
  expect(data.total.costUsd).toBe(6); expect(data.total.unpricedTokens).toBe(2300); expect(costLabel(data.total)).toBe('≥ $6.00');
});
it('marks partial native subagent evidence as a lower bound even when parent tokens are priced', () => {
  const stats = summary(); stats.totals.unpricedTokens = 0; stats.totals.usageTurns = stats.totals.turns; stats.totals.partialUsageTurns = 1;
  stats.byModel[0]!.partialUsageTurns = 1;
  const data = accountUsage([{ deviceId: 'a', name: 'A', summary: stats }], index, 'today', 'cost');
  expect(data.total.tokenUnknown).toBe(true); expect(costLabel(data.total)).toBe('≥ $6.00');
  expect(data.models.find(row => row.id === 'Fast')!.partial).toBe(true);
});
