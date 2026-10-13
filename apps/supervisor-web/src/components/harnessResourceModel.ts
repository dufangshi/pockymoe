import { translate } from '@pockymoe/thread-ui/i18n';
import type { HarnessBusyReason, HarnessProcessDto, LoadedThreadDto, ProcessMemoryMetric } from '@pockymoe/shared';

const GIB = 2 ** 30;

/** Process memory: MiB below a GiB. */
export const memory = (bytes: number | null) =>
  bytes == null ? '—' : bytes >= GIB ? `${(bytes / GIB).toFixed(1)} GiB` : `${Math.round(bytes / 2 ** 20)} MiB`;
export const METRIC_LABEL: Record<ProcessMemoryMetric, () => string> = {
  pss: () => translate('devices.memoryMetricPss'),
  physFootprint: () => translate('devices.memoryMetricPhysFootprint'),
  privateWorkingSet: () => translate('devices.memoryMetricPrivateWorkingSet'),
  rss: () => translate('devices.memoryMetricRss'),
};
const BUSY_LABEL: Record<HarnessBusyReason, () => string> = {
  turn: () => translate('devices.harnessRunning'),
  pendingRequest: () => translate('devices.harnessBusyPendingRequest'),
  queuedInput: () => translate('devices.harnessBusyQueuedInput'),
  backgroundTask: () => translate('devices.harnessBusyBackgroundTask'),
  subagent: () => translate('devices.harnessBusySubagent'),
  goal: () => translate('devices.harnessBusyGoal'),
};
export const ROLE_LABEL: Record<HarnessProcessDto['role'], () => string> = {
  adapter: () => translate('devices.processRoleAdapter'),
  agent: () => translate('devices.processRoleAgent'),
  bridge: () => translate('devices.processRoleBridge'),
  mcp: () => translate('devices.processRoleMcp'),
  tool: () => translate('devices.processRoleTool'),
  other: () => translate('devices.processRoleOther'),
};

/** Running, a busy reason, viewing, or idle minutes since the last activity. */
export function harnessState(thread: LoadedThreadDto, now: number) {
  const reason = thread.busy.includes('turn') ? 'turn' : thread.busy[0];
  if (reason) return BUSY_LABEL[reason]?.() ?? reason;
  if (thread.displayed) return translate('devices.harnessViewing');
  const minutes = Math.floor((now - Date.parse(thread.lastActivityAt)) / 60_000);
  return Number.isFinite(minutes) && minutes >= 1
    ? translate('devices.harnessIdleMinutes', { count: minutes })
    : translate('devices.harnessIdle');
}

/** The Supervisor's accepted ranges for each setting. */
export const RESOURCE_LIMITS = {
  idleReleaseMinutes: { min: 1, max: 1440 },
  maxLoadedThreads: { min: 1, max: 200 },
  maxHarnessMemoryBytes: { min: 256 * 2 ** 20 },
} as const;

export type ResourceField = keyof typeof RESOURCE_LIMITS;

export const gibText = (bytes: number) => String(Number((bytes / GIB).toFixed(2)));

/** The value to save for one field, or `undefined` when it is out of range. */
export function parseResourceField(field: ResourceField, on: boolean, text: string, saved?: number | null): number | null | undefined {
  if (!on) return null;
  const value = Number(text.trim());
  if (!text.trim() || !Number.isFinite(value)) return undefined;
  if (field === 'maxHarnessMemoryBytes') {
    // An untouched value keeps its exact bytes rather than the rounded GiB shown.
    const bytes = saved != null && gibText(saved) === text.trim() ? saved : Math.round(value * GIB);
    return bytes >= RESOURCE_LIMITS.maxHarnessMemoryBytes.min ? bytes : undefined;
  }
  const { min, max } = RESOURCE_LIMITS[field];
  return Number.isInteger(value) && value >= min && value <= max ? value : undefined;
}
