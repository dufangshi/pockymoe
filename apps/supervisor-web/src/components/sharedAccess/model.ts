import type {
  RelayAccessGrantDto,
  RelayPortalSummaryDto,
  RelaySessionShareDto,
  RelayThreadAccessDto,
  RelayWorkspaceAccessDto,
} from '@pockymoe/shared';

export type ShareDirection = 'in' | 'out';
export type ShareScope = 'device' | 'workspace' | 'thread';
export type ShareFilter = 'all' | ShareScope | 'attention';
export type SharedView = 'list' | 'devices' | 'people';

/** One share or grant, either direction, in the fields every view reads. */
export interface SharedItem {
  key: string;
  direction: ShareDirection;
  scope: ShareScope;
  title: string;
  workspaceId: string | null;
  workspaceLabel: string | null;
  deviceId: string;
  deviceName: string;
  deviceConnected: boolean | undefined;
  /** The other side: who shared it with me, or whom I shared it with. */
  personId: string;
  personName: string;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  canCreateThreads: boolean;
  label: string | null;
  createdAt: string;
  expiresAt: string | null;
  lastAccessedAt: string | null;
  /** Whole-device grants can name selected workspaces only. */
  selectedWorkspaces: number | null;
  source: { kind: 'share'; row: RelaySessionShareDto } | { kind: 'grant'; row: RelayAccessGrantDto };
}

const clean = (value: string | null | undefined) => value?.trim() || null;

function fromShare(row: RelaySessionShareDto, direction: ShareDirection): SharedItem {
  return {
    key: `share:${row.id}`,
    direction,
    scope: 'thread',
    title: clean(row.threadTitle) ?? clean(row.label) ?? row.threadId.slice(0, 8),
    workspaceId: row.workspaceId,
    workspaceLabel: clean(row.workspaceLabel),
    deviceId: row.deviceId,
    deviceName: clean(row.deviceName) ?? row.deviceId.slice(0, 8),
    deviceConnected: row.deviceConnected,
    personId: direction === 'in' ? row.ownerUserId : row.targetUserId,
    personName: direction === 'in' ? row.ownerUsername : row.targetUsername,
    threadAccess: row.threadAccess,
    workspaceAccess: row.workspaceAccess,
    canCreateThreads: false,
    label: clean(row.label),
    createdAt: row.createdAt,
    expiresAt: row.expiresAt,
    lastAccessedAt: row.lastAccessedAt,
    selectedWorkspaces: null,
    source: { kind: 'share', row },
  };
}

function fromGrant(row: RelayAccessGrantDto, direction: ShareDirection): SharedItem {
  const deviceName = clean(row.deviceName) ?? row.deviceId.slice(0, 8);
  return {
    key: `grant:${row.id}`,
    direction,
    scope: row.scope,
    title: row.scope === 'thread'
      ? clean(row.threadTitle) ?? clean(row.label) ?? (row.threadId ?? '').slice(0, 8)
      : row.scope === 'workspace'
        ? clean(row.workspaceLabel) ?? (row.workspaceId ?? '').slice(0, 8)
        : deviceName,
    workspaceId: row.workspaceId,
    workspaceLabel: clean(row.workspaceLabel),
    deviceId: row.deviceId,
    deviceName,
    deviceConnected: row.deviceConnected,
    personId: direction === 'in' ? row.ownerUserId : row.targetUserId,
    personName: direction === 'in' ? row.ownerUsername : row.targetUsername,
    threadAccess: row.threadAccess,
    workspaceAccess: row.workspaceAccess,
    canCreateThreads: row.canCreateThreads,
    label: clean(row.label),
    createdAt: row.createdAt,
    expiresAt: row.expiresAt,
    lastAccessedAt: row.lastAccessedAt,
    selectedWorkspaces: row.scope === 'device' && row.workspaceScope === 'selected' ? row.workspaceIds.length : null,
    source: { kind: 'grant', row },
  };
}

const activity = (item: SharedItem) => item.lastAccessedAt ?? item.createdAt;

/** Everything shared in one direction, most recently used first. */
export function sharedItems(portal: RelayPortalSummaryDto | null, direction: ShareDirection): SharedItem[] {
  if (!portal) return [];
  const items = direction === 'in'
    ? [
      ...portal.sharedWithMe.map(row => fromShare(row, 'in')),
      ...(portal.sharedDevicesWithMe ?? []).map(row => fromGrant(row, 'in')),
      ...(portal.sharedThreadsWithMe ?? []).map(row => fromGrant(row, 'in')),
    ]
    : [
      ...portal.sharedByMe.map(row => fromShare(row, 'out')),
      ...(portal.grantsByMe ?? []).map(row => fromGrant(row, 'out')),
    ];
  const unique = new Map(items.map(item => [item.key, item]));
  return [...unique.values()].sort((a, b) => activity(b).localeCompare(activity(a)) || a.title.localeCompare(b.title));
}

export function matchesSearch(item: SharedItem, query: string) {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return true;
  return [item.title, item.workspaceLabel, item.deviceName, item.personName, item.label]
    .some(value => value?.toLocaleLowerCase().includes(needle));
}

export function filterItems(items: SharedItem[], filter: ShareFilter, query: string, attention: ReadonlyMap<string, unknown>) {
  return items.filter(item =>
    (filter === 'all' || (filter === 'attention' ? attention.has(item.key) : item.scope === filter))
    && matchesSearch(item, query));
}

export function scopeCounts(items: SharedItem[]) {
  const counts: Record<ShareScope, number> = { device: 0, workspace: 0, thread: 0 };
  for (const item of items) counts[item.scope] += 1;
  return counts;
}

const SCOPE_ORDER: Record<ShareScope, number> = { device: 0, workspace: 1, thread: 2 };

/** A device's shares as a tree: the whole device, then each workspace (shared
    itself, or only through its threads) with the threads shared in it. */
export interface DeviceNode {
  deviceId: string;
  deviceName: string;
  deviceConnected: boolean | undefined;
  people: string[];
  items: SharedItem[];
  rows: Array<
    | { type: 'item'; depth: 0 | 1; item: SharedItem }
    | { type: 'workspace'; depth: 0; label: string }
  >;
}

export function groupByDevice(items: SharedItem[]): DeviceNode[] {
  const devices = new Map<string, SharedItem[]>();
  for (const item of items) devices.set(item.deviceId, [...(devices.get(item.deviceId) ?? []), item]);
  return [...devices].map(([deviceId, deviceItems]) => {
    const sorted = [...deviceItems].sort((a, b) =>
      SCOPE_ORDER[a.scope] - SCOPE_ORDER[b.scope] || a.title.localeCompare(b.title) || a.personName.localeCompare(b.personName));
    const rows: DeviceNode['rows'] = sorted.filter(item => item.scope === 'device').map(item => ({ type: 'item', depth: 0, item }));
    const workspaceKey = (item: SharedItem) => item.workspaceId ?? item.workspaceLabel;
    const workspaces = new Map<string, { label: string; shared: SharedItem[]; threads: SharedItem[] }>();
    const loose: SharedItem[] = [];
    for (const item of sorted) {
      if (item.scope === 'device') continue;
      const key = workspaceKey(item);
      if (!key) { loose.push(item); continue; }
      const entry = workspaces.get(key) ?? { label: item.workspaceLabel ?? key.slice(0, 8), shared: [], threads: [] };
      (item.scope === 'workspace' ? entry.shared : entry.threads).push(item);
      workspaces.set(key, entry);
    }
    for (const entry of [...workspaces.values()].sort((a, b) => a.label.localeCompare(b.label))) {
      if (entry.shared.length) rows.push(...entry.shared.map(item => ({ type: 'item' as const, depth: 0 as const, item })));
      else rows.push({ type: 'workspace', depth: 0, label: entry.label });
      rows.push(...entry.threads.map(item => ({ type: 'item' as const, depth: 1 as const, item })));
    }
    rows.push(...loose.map(item => ({ type: 'item' as const, depth: 0 as const, item })));
    const first = deviceItems[0]!;
    return {
      deviceId,
      deviceName: first.deviceName,
      deviceConnected: deviceItems.some(item => item.deviceConnected === true) ? true : first.deviceConnected,
      people: [...new Set(deviceItems.map(item => item.personName))],
      items: deviceItems,
      rows,
    };
  }).sort((a, b) => a.deviceName.localeCompare(b.deviceName));
}

export interface PersonNode {
  personId: string;
  personName: string;
  counts: Record<ShareScope, number>;
  items: SharedItem[];
  lastAccessedAt: string | null;
}

export function groupByPerson(items: SharedItem[]): PersonNode[] {
  const people = new Map<string, SharedItem[]>();
  for (const item of items) people.set(item.personId, [...(people.get(item.personId) ?? []), item]);
  return [...people].map(([personId, personItems]) => ({
    personId,
    personName: personItems[0]!.personName,
    counts: scopeCounts(personItems),
    items: [...personItems].sort((a, b) => SCOPE_ORDER[a.scope] - SCOPE_ORDER[b.scope] || a.title.localeCompare(b.title)),
    lastAccessedAt: personItems.map(item => item.lastAccessedAt).filter((value): value is string => Boolean(value)).sort().at(-1) ?? null,
  })).sort((a, b) => a.personName.localeCompare(b.personName));
}

export const initials = (name: string) => Array.from(name.trim() || '?').slice(0, 2).join('').toUpperCase();
