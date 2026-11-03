import type { ThreadActivityResponseDto } from '@pockymoe/shared';
import { formatNumber, translate } from '@pockymoe/thread-ui/i18n';
import { running, type DesktopIndex } from '../data/types';

export type StatsRange = 'today' | '7d' | '30d' | 'all';
export type TurnOrigin = 'owner' | 'guest' | 'agent' | 'automation' | 'unknown';
export interface UsageValues {
  turns: number; tokens: number; costUsd: number | null; unpricedTokens?: number; usageTurns?: number; partialUsageTurns?: number; requests?: number | null; knownRequests?: number;
  inputTokens?: number; cachedTokens?: number; cacheWriteTokens?: number; outputTokens?: number; reasoningTokens?: number;
}
export interface HourBucket extends UsageValues { hour: number; failed: number }
export interface StatsSummary {
  range: StatsRange;
  generatedAt: string;
  origin?: TurnOrigin | 'all';
  coverage?: { missingUsageTurns?: number; partialUsageTurns?: number; undatedTurns?: number; unknownOriginTurns?: number; excludedUndatedTurns?: number; excludedHistoryCopies?: number };
  totals: {
    turns: number; completed: number; failed: number; interrupted: number;
    avgDurationMs: number | null; p50DurationMs: number | null; durationSamples?: number | undefined;
    inputTokens: number; outputTokens: number; cachedTokens: number; costUsd: number | null; unpricedTokens?: number; tokens?: number; usageTurns?: number; partialUsageTurns?: number; cacheWriteTokens?: number; reasoningTokens?: number; knownRequests?: number; requests?: number | null;
  };
  daily: ({ date: string; failed: number } & UsageValues)[];
  hourly?: HourBucket[];
  byModel: ({ model: string } & UsageValues)[];
  byWorkspace: ({ workspaceId: string; label: string; lastActivityAt: string | null } & UsageValues)[];
  longestThreads: { threadId: string; title: string; workspaceId: string; durationMs: number }[];
  byOrigin?: ({ origin: TurnOrigin } & UsageValues)[];
  activeThreads: number;
  runningTurns: number;
}
export interface DeviceMetrics {
  summaries: Partial<Record<StatsRange, StatsSummary>>;
  originSummaries?: Partial<Record<TurnOrigin, Partial<Record<StatsRange, StatsSummary>>>>;
  partial?: boolean;
  unavailable?: boolean;
  allUnsupported?: boolean;
  version?: string | undefined;
  latestVersion?: string | undefined;
  latencyMs?: number | undefined;
  checkedAt?: string;
  activity?: ThreadActivityResponseDto;
}
export const compact = (value: number | null | undefined) => value == null ? '—' : formatNumber(value, { notation: 'compact', maximumFractionDigits: 1 });
export const exact = (value: number) => formatNumber(value);
export const money = (value: number | null | undefined) => value == null ? '—' : `$${formatNumber(value, { ...(Math.abs(value) >= 1000 ? { notation: 'compact' as const } : {}), minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
export const turns = (value: number) => translate('desktopDevices.turnCount', { count: value, value: exact(value) });
export function duration(value: number | null | undefined) {
  if (value == null) return '—';
  const seconds = Math.round(value / 1000);
  return seconds < 60 ? translate('desktopDevices.durationSeconds', { seconds }) : seconds < 3600 ? translate('desktopDevices.durationMinutes', { minutes: Math.floor(seconds / 60), seconds: seconds % 60 }) : translate('desktopDevices.durationHours', { hours: Math.floor(seconds / 3600), minutes: Math.floor(seconds % 3600 / 60) });
}
export function age(value: string | null | undefined, now = Date.now()) {
  if (!value || !Number.isFinite(Date.parse(value))) return translate('desktopDevices.unknownTime');
  const minutes = Math.max(0, Math.floor((now - Date.parse(value)) / 60_000));
  return minutes < 1 ? translate('desktopDevices.justNow') : minutes < 60 ? translate('desktopDevices.minutesAgo', { count: minutes }) : minutes < 1440 ? translate('desktopDevices.hoursAgo', { count: Math.floor(minutes / 60) }) : translate('desktopDevices.daysAgo', { count: Math.floor(minutes / 1440) });
}
const dateFormats = new Map<string, Intl.DateTimeFormat>();
export function dateKey(value: Date, timeZone: string) {
  let formatter = dateFormats.get(timeZone);
  if (!formatter) { formatter = new Intl.DateTimeFormat('en-US', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }); dateFormats.set(timeZone, formatter); }
  const parts = formatter.formatToParts(value);
  const part = (type: string) => parts.find(p => p.type === type)!.value;
  return `${part('year')}-${part('month')}-${part('day')}`;
}
export function calendarDays(range: StatsRange, timeZone: string, now = new Date()) {
  const today = dateKey(now, timeZone);
  const count = range === 'today' ? 1 : range === '7d' ? 7 : 30;
  const end = new Date(`${today}T12:00:00Z`);
  return Array.from({ length: count }, (_, i) => {
    const day = new Date(end); day.setUTCDate(day.getUTCDate() - count + i + 1);
    return day.toISOString().slice(0, 10);
  });
}
export function deviceCounts(index: DesktopIndex, deviceId: string) {
  const threads = index.threads.filter(t => t.deviceId === deviceId);
  const conversation = (thread: typeof threads[number]) => thread.rootThreadId || thread.parentThreadId || thread.id;
  return { running: new Set(threads.filter(running).map(conversation)).size, attention: new Set(threads.filter(t => t.requests.length > 0).map(conversation)).size,
    workspaces: index.workspaces.filter(w => w.deviceId === deviceId).length, threads: new Set(threads.map(conversation)).size };
}
/** The index only knows the latest turn per thread. Never present it as full history. */
export function partialSummary(index: DesktopIndex, deviceId: string, range: StatsRange, timeZone: string, now = new Date()): StatsSummary {
  const dates = range === 'all' ? [...new Set(index.threads.filter(t => t.deviceId === deviceId && t.lastTurnStartedAt && Number.isFinite(Date.parse(t.lastTurnStartedAt))).map(t => dateKey(new Date(t.lastTurnStartedAt!), timeZone)))].sort() : calendarDays(range, timeZone, now);
  const daily = dates.map(date => ({ date, turns: 0, failed: 0, tokens: 0, costUsd: null as number | null }));
  const threads = index.threads.filter(t => t.deviceId === deviceId);
  let completed = 0, failed = 0, interrupted = 0;
  const workspaces = new Map<string, StatsSummary['byWorkspace'][number]>();
  for (const thread of threads) {
    const start = thread.lastTurnStartedAt && new Date(thread.lastTurnStartedAt);
    if (!start || !Number.isFinite(+start) || +start > +now) continue;
    const day = daily.find(day => day.date === dateKey(start, timeZone));
    if (!day) continue;
    day.turns++;
    const ended = thread.lastTurnCompletedAt && Date.parse(thread.lastTurnCompletedAt) >= +start;
    if (ended && (thread.status === 'failed' || thread.status === 'system_error')) { failed++; day.failed++; }
    else if (ended && thread.status === 'interrupted') interrupted++;
    else if (ended && !running(thread)) completed++;
    const row = workspaces.get(thread.workspaceId) ?? { workspaceId: thread.workspaceId, label: index.workspaces.find(w => w.deviceId === deviceId && w.id === thread.workspaceId)?.label ?? translate('desktopDevices.workspace'), turns: 0, tokens: 0, costUsd: null, lastActivityAt: thread.lastTurnStartedAt! };
    row.turns++; row.lastActivityAt = (row.lastActivityAt ?? '') > thread.updatedAt ? row.lastActivityAt : thread.updatedAt;
    workspaces.set(row.workspaceId, row);
  }
  return { range, generatedAt: now.toISOString(), daily, totals: { turns: daily.reduce((n, d) => n + d.turns, 0), completed, failed, interrupted, avgDurationMs: null, p50DurationMs: null, inputTokens: 0, outputTokens: 0, cachedTokens: 0, costUsd: null }, byModel: [], byWorkspace: [...workspaces.values()].sort((a, b) => b.turns - a.turns), longestThreads: [], activeThreads: threads.filter(running).length, runningTurns: threads.filter(t => running(t) && t.activeTurnId).length };
}
const sumCost = (values: (number | null)[]) => values.some(v => v == null) ? null : values.reduce<number>((n, v) => n + v!, 0);
const pricePartial = (rows: UsageValues[]) => rows.some(row => row.costUsd == null || row.unpricedTokens || row.partialUsageTurns || row.usageTurns !== undefined && row.usageTurns < row.turns);
function bucketCost(rows: UsageValues[]) {
  const known = rows.filter(row => row.costUsd != null);
  const evidence = known.some(row => row.turns > 0 || row.tokens > 0 || row.costUsd! > 0);
  return known.length && (evidence || known.length === rows.length) ? known.reduce((sum, row) => sum + row.costUsd!, 0) : null;
}
export function aggregate(sources: { deviceId: string; summary: StatsSummary }[]) {
  const summaries = sources.map(s => s.summary);
  const sum = (key: 'turns' | 'completed' | 'failed' | 'interrupted' | 'inputTokens' | 'outputTokens' | 'cachedTokens') => summaries.reduce((n, s) => n + s.totals[key], 0);
  const terminal = sum('completed') + sum('failed') + sum('interrupted');
  const nonempty = summaries.filter(s => s.totals.turns > 0);
  const samples = summaries.reduce((n, s) => n + (s.totals.durationSamples ?? 0), 0);
  const mean = nonempty.length === 1 ? nonempty[0]!.totals.avgDurationMs : nonempty.every(s => s.totals.durationSamples !== undefined) && samples > 0 ? summaries.reduce((n, s) => n + (s.totals.avgDurationMs ?? 0) * (s.totals.durationSamples ?? 0), 0) / samples : null;
  const daily = [...new Set(summaries.flatMap(s => s.daily.map(d => d.date)))].sort().map(date => {
    const rows = summaries.flatMap(s => s.daily.filter(d => d.date === date));
    return { date, turns: rows.reduce((n, d) => n + d.turns, 0), tokens: rows.reduce((n, d) => n + d.tokens, 0), costUsd: bucketCost(rows), pricePartial: pricePartial(rows) };
  });
  const models = new Map<string, { model: string; turns: number; tokens: number }>();
  summaries.flatMap(s => s.byModel).forEach(row => { const old = models.get(row.model); models.set(row.model, { model: row.model, turns: (old?.turns ?? 0) + row.turns, tokens: (old?.tokens ?? 0) + row.tokens }); });
  return { turns: sum('turns'), successRate: terminal ? sum('completed') / terminal : null, avgDurationMs: mean,
    tokens: sum('inputTokens') + sum('outputTokens'), cachedTokens: sum('cachedTokens'), costUsd: sumCost(summaries.map(s => s.totals.costUsd)), daily,
    models: [...models.values()].sort((a, b) => b.turns - a.turns),
    workspaces: sources.flatMap(s => s.summary.byWorkspace.map(w => ({ ...w, deviceId: s.deviceId }))).sort((a, b) => b.turns - a.turns).slice(0, 5) };
}
export function supportsStats(health: { capabilities?: { statsSummary?: boolean } }) { return health.capabilities?.statsSummary === true; }
export function supportsHourly(summary: StatsSummary | undefined): summary is StatsSummary & { hourly: HourBucket[] } {
  const rows = summary?.hourly;
  return summary?.range === 'today' && Array.isArray(rows) && rows.length === 24 && Array.from(rows).every(row => row && Number.isInteger(row.hour) && row.hour >= 0 && row.hour < 24 && [row.turns, row.failed, row.tokens].every(value => Number.isFinite(value) && value >= 0) && (row.costUsd === null || Number.isFinite(row.costUsd) && row.costUsd >= 0)) && new Set(rows.map(row => row.hour)).size === 24;
}
export function aggregateHourly(sources: { summary: StatsSummary }[]) {
  return Array.from({ length: 24 }, (_, hour) => {
    const rows = sources.flatMap(source => supportsHourly(source.summary) ? [source.summary.hourly.find(row => row.hour === hour)!] : []);
    const known = rows.length > 0;
    return { hour, tokens: known ? rows.reduce((sum, row) => sum + row.tokens, 0) : null,
      costUsd: known ? bucketCost(rows) : null, pricePartial: known && pricePartial(rows) };
  });
}

/** Activity bins roll from `since`, rather than from local clock boundaries.
 *  Overlap weighting is an explicit estimate, never used as turn/token usage. */
export function runtimeHours(activity: ThreadActivityResponseDto | undefined, timeZone: string, now = new Date()): (number | null)[] {
  if (!activity || !Number.isFinite(Date.parse(activity.since)) || !Number.isInteger(activity.hours) || activity.hours < 1 || activity.hours > 168) return Array(24).fill(null);
  const since = Date.parse(activity.since), end = Math.min(+now, since + activity.hours * 3_600_000);
  if (end <= since || activity.threads.some(row => row.activeMs > 0 && (!row.hourlyMs || row.hourlyMs.length !== activity.hours || row.hourlyMs.some(ms => !Number.isFinite(ms) || ms < 0)))) return Array(24).fill(null);
  const today = dateKey(now, timeZone);
  const values: (number | null)[] = Array(24).fill(null);
  const clock = new Intl.DateTimeFormat('en-GB', { timeZone, hour: '2-digit', hourCycle: 'h23', minute: '2-digit', second: '2-digit' });
  for (let bin = 0; bin < activity.hours; bin++) {
    const from = since + bin * 3_600_000, to = Math.min(end, from + 3_600_000);
    if (to <= from) continue;
    const total = activity.threads.reduce((sum, row) => sum + Math.max(0, row.hourlyMs?.[bin] ?? 0), 0);
    for (let at = from; at < to;) {
      const parts = clock.formatToParts(new Date(at));
      const minutes = Number(parts.find(part => part.type === 'minute')!.value), seconds = Number(parts.find(part => part.type === 'second')!.value);
      const next = Math.min(to, at + (60 - minutes) * 60_000 - seconds * 1000 - new Date(at).getUTCMilliseconds());
      if (dateKey(new Date(at), timeZone) === today) {
        const hour = Number(parts.find(part => part.type === 'hour')!.value);
        values[hour] = (values[hour] ?? 0) + total * (next - at) / 3_600_000;
      }
      at = next;
    }
  }
  return values;
}
export function isUnsupported(error: unknown) { return typeof error === 'object' && error !== null && 'statusCode' in error && error.statusCode === 404; }
export function newerVersion(current: string | undefined, latest: string | undefined) {
  const parse = (value: string | undefined) => value?.replace(/^v/, '').match(/^(\d+)\.(\d+)\.(\d+)$/)?.slice(1).map(Number);
  const a = parse(current), b = parse(latest);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i++) { if (a[i] !== b[i]) return b[i]! > a[i]!; }
  return false;
}

/** thread-activity exposes lifetime usage, even when its runtime window is short. */
export function lifetimeActivity(index: DesktopIndex, deviceId: string, activity: ThreadActivityResponseDto) {
  const known = new Map(index.threads.filter(t => t.deviceId === deviceId).map(t => [t.id, t]));
  const rows = activity.threads.filter(row => known.has(row.threadId));
  const workspaces = new Map<string, { id: string; label: string; turns: number }>();
  rows.forEach(row => {
    const thread = known.get(row.threadId)!;
    const old = workspaces.get(thread.workspaceId) ?? { id: thread.workspaceId, label: index.workspaces.find(w => w.deviceId === deviceId && w.id === thread.workspaceId)?.label ?? translate('desktopDevices.workspace'), turns: 0 };
    old.turns += row.turns; workspaces.set(old.id, old);
  });
  return { turns: rows.reduce((n, row) => n + row.turns, 0),
    tokens: rows.some(row => !row.tokenUsage) ? null : rows.reduce((n, row) => n + row.tokenUsage!.inputTokens + row.tokenUsage!.outputTokens, 0),
    costUsd: rows.some(row => row.pricePartial || !row.priceEstimate) ? null : rows.reduce((n, row) => n + row.priceEstimate!.totalUsd, 0),
    workspaces: [...workspaces.values()].sort((a, b) => b.turns - a.turns).slice(0, 5) };
}
/** Index the full account inventory, never a metric-sorted or filtered subset. */
export function categoryColor(id: string, ids: readonly string[]) {
  const position = Math.max(0, [...new Set(ids)].sort().indexOf(id));
  const base = `var(--usage-category-${position % 12 + 1})`;
  const cycle = Math.floor(position / 12);
  return cycle ? `color-mix(in oklch, ${base} ${100 / (cycle + 1)}%, var(--theme-fg))` : base;
}
