import { translate } from '@pockymoe/thread-ui/i18n';
import type { ThreadActivityResponseDto } from '@pockymoe/shared';
import { entityKey, type DesktopIndex } from '../data/types';
import { money, type StatsRange, type StatsSummary, type TurnOrigin, type UsageValues } from './model';

type ActivityRow = ThreadActivityResponseDto['threads'][number] & { byOrigin?: Partial<Record<TurnOrigin, number>>; usageCoverage?: { missingUsageTurns?: number; partialUsageTurns?: number; unpricedTokens?: number } };
export type UsageMetric = 'tokens' | 'cost';
export interface UsageRow extends UsageValues { id: string; label: string; partial: boolean; unknown?: boolean; tokenUnknown?: boolean; hasPrice?: boolean }
export interface DeviceUsage extends UsageRow { workspaces: UsageRow[]; models: UsageRow[]; modelHistory: boolean }
export interface UsageSource { deviceId: string; name: string; summary?: StatsSummary | undefined; activity?: ThreadActivityResponseDto | undefined }
export const metricValue = (row: UsageValues, metric: UsageMetric) => metric === 'tokens' ? row.tokens : row.costUsd ?? 0;
/** Historical adapters sometimes persisted a serialized provider/model tuple. */
export function normalizeModelName(raw: string): string {
  try {
    const value: unknown = JSON.parse(raw);
    if (Array.isArray(value) && value.length && typeof value.at(-1) === 'string') return value.at(-1) as string;
  } catch { /* Ordinary model IDs are already display names. */ }
  return raw;
}
export function modelLabel(id: string) {
  return id === '<synthetic>' ? translate('desktopDevices.systemGenerated') : id === 'unknown' ? translate('desktopDevices.unknownModel') : id;
}
export const costLabel = (row: Pick<UsageValues, 'costUsd' | 'unpricedTokens'> & { partial?: boolean }) => row.costUsd == null ? '—' : `${row.unpricedTokens || row.partial ? '≥ ' : ''}${money(row.costUsd)}`;
const empty = (id: string, label = id): UsageRow => ({ id, label, turns: 0, tokens: 0, costUsd: null, unpricedTokens: 0, partial: false });
const incompleteUsage = (row: UsageValues) => Boolean(row.partialUsageTurns || row.usageTurns !== undefined && row.usageTurns < row.turns);
const fields = ['inputTokens', 'cachedTokens', 'cacheWriteTokens', 'outputTokens', 'reasoningTokens'] as const;
function add(target: UsageRow, row: UsageValues & { hasPrice?: boolean }, partial = false, tokenUnknown = false) {
  target.hasPrice ||= row.hasPrice ?? (row.costUsd != null && (row.turns > 0 || row.tokens > 0));
  target.tokenUnknown ||= tokenUnknown;
  target.turns += row.turns; target.tokens += row.tokens;
  if (row.costUsd != null) target.costUsd = (target.costUsd ?? 0) + row.costUsd;
  if (target.costUsd === 0 && target.turns > 0 && !target.hasPrice) target.costUsd = null;
  target.unpricedTokens = (target.unpricedTokens ?? 0) + (row.unpricedTokens ?? (row.costUsd == null ? row.tokens : 0));
  target.partial ||= partial || row.costUsd == null && row.tokens > 0;
  for (const field of fields) if (row[field] !== undefined) target[field] = (target[field] ?? 0) + row[field]!;
}
export function accountUsage(sources: UsageSource[], index: DesktopIndex, range: StatsRange, metric: UsageMetric, origin: TurnOrigin | '' = '') {
  const models = new Map<string, UsageRow>();
  const threads = new Map(index.threads.map(thread => [thread.key, thread]));
  const workspaceLabels = new Map(index.workspaces.map(workspace => [workspace.key, workspace.label]));
  const devices: DeviceUsage[] = [];
  let partial = false;
  const origins = { owner: 0, guest: 0, agent: 0, automation: 0, unknown: 0 };
  for (const source of sources) {
    const device: DeviceUsage = { ...empty(source.deviceId, source.name), workspaces: [], models: [], modelHistory: false };
    if (source.summary && (!origin || source.summary.origin === origin)) {
      const summary = source.summary;
      const totals = { ...summary.totals, tokens: summary.totals.tokens ?? summary.totals.inputTokens + summary.totals.outputTokens };
      const pricedModels = summary.byModel.filter(row => row.costUsd != null);
      const recoveredCost = totals.costUsd == null && pricedModels.length > 0;
      if (recoveredCost) totals.costUsd = pricedModels.reduce((sum, row) => sum + row.costUsd!, 0);
      totals.unpricedTokens ??= summary.byModel.reduce((sum, row) => sum + (row.unpricedTokens ?? (row.costUsd == null ? row.tokens : 0)), 0);
      const incomplete = recoveredCost || Boolean(summary.coverage?.missingUsageTurns || summary.coverage?.partialUsageTurns || summary.totals.partialUsageTurns || summary.coverage?.excludedUndatedTurns);
      add(device, totals, incomplete, Boolean(summary.coverage?.missingUsageTurns || summary.coverage?.partialUsageTurns || summary.totals.partialUsageTurns)); partial ||= incomplete;
      device.modelHistory = true;
      const deviceModels = new Map<string, UsageRow>();
      for (const row of summary.byModel) {
        const id = normalizeModelName(row.model);
        for (const group of [models, deviceModels]) {
          const target = group.get(id) ?? empty(id, modelLabel(id));
          add(target, row, incompleteUsage(row), incompleteUsage(row)); group.set(id, target);
        }
      }
      device.models = [...deviceModels.values()].sort((a, b) => metricValue(b, metric) - metricValue(a, metric) || a.label.localeCompare(b.label));
      device.workspaces = summary.byWorkspace.map(row => { const target = empty(row.workspaceId, row.label); add(target, row, incompleteUsage(row), incompleteUsage(row)); return target; });
      for (const row of summary.byOrigin ?? []) if (row.origin in origins) origins[row.origin] += row.turns;
    } else if (!source.summary && source.activity && range === 'all' && !origin) {
      // Activity tokens are lifetime and lack per-model history. Never attribute
      // historical usage to the thread's current model, or to a finite date range.
      device.partial = true; partial = true;
      const workspaces = new Map<string, UsageRow>();
      for (const row of source.activity.threads as ActivityRow[]) {
        const thread = threads.get(entityKey(source.deviceId, row.threadId));
        const usage: UsageValues = { turns: row.turns, tokens: row.tokenUsage ? row.tokenUsage.totalTokens ?? row.tokenUsage.inputTokens + row.tokenUsage.outputTokens : 0, costUsd: row.priceEstimate?.totalUsd ?? null, ...(row.usageCoverage?.unpricedTokens !== undefined ? { unpricedTokens: row.usageCoverage.unpricedTokens } : {}), ...(row.tokenUsage ? { inputTokens: row.tokenUsage.inputTokens, outputTokens: row.tokenUsage.outputTokens, cachedTokens: row.tokenUsage.cachedInputTokens, reasoningTokens: row.tokenUsage.reasoningOutputTokens, ...(row.tokenUsage.cacheWriteInputTokens !== undefined ? { cacheWriteTokens: row.tokenUsage.cacheWriteInputTokens } : {}) } : {}) };
        const target = workspaces.get(thread?.workspaceId ?? '') ?? empty(thread?.workspaceId ?? '', (thread ? workspaceLabels.get(entityKey(source.deviceId, thread.workspaceId)) : undefined) ?? '—');
        const tokenUnknown = Boolean(!row.tokenUsage || row.usageCoverage?.missingUsageTurns || row.usageCoverage?.partialUsageTurns);
        const pricePartial = Boolean(row.pricePartial) || tokenUnknown;
        const model = models.get('unknown') ?? empty('unknown', translate('desktopDevices.unknownModel'));
        add(model, usage, pricePartial, tokenUnknown); models.set('unknown', model);
        add(target, usage, pricePartial, tokenUnknown); add(device, usage, pricePartial, tokenUnknown); workspaces.set(target.id, target);
        let counted = 0;
        for (const key of Object.keys(origins) as TurnOrigin[]) { const count = row.byOrigin?.[key] ?? 0; origins[key] += count; counted += count; }
        origins.unknown += Math.max(0, row.turns - counted);
      }
      device.workspaces = [...workspaces.values()];
    } else { device.partial = true; device.unknown = true; partial = true; }
    device.workspaces.sort((a, b) => metricValue(b, metric) - metricValue(a, metric) || a.label.localeCompare(b.label));
    devices.push(device);
  }
  const ordered = [...models.values()].sort((a, b) => metricValue(b, metric) - metricValue(a, metric) || a.label.localeCompare(b.label));
  devices.sort((a, b) => metricValue(b, metric) - metricValue(a, metric) || a.label.localeCompare(b.label));
  const total = empty('total'); devices.forEach(row => add(total, row, row.partial, Boolean(row.tokenUnknown))); total.unknown = devices.every(row => row.unknown);
  return { models: ordered, devices, total, partial, origins };
}
