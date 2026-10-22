import { describe, expect, it } from 'vitest';
import type { RelayAccessGrantDto, RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';
import { filterItems, groupByDevice, groupByPerson, sharedItems } from './model';

const share = (id: string, over: Partial<RelaySessionShareDto> = {}): RelaySessionShareDto => ({
  id, ownerUserId: 'me', ownerUsername: 'du', targetUserId: 'u-joe', targetUsername: 'joe',
  deviceId: 'dev-1', deviceName: 'Supervisor', deviceConnected: true, threadId: `t-${id}`, threadTitle: `Thread ${id}`,
  workspaceId: 'ws-1', workspaceLabel: 'creativocli', label: null, threadAccess: 'read', workspaceAccess: 'read',
  createdAt: '2026-10-01T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [],
  ...over,
});
const grant = (id: string, scope: RelayAccessGrantDto['scope'], over: Partial<RelayAccessGrantDto> = {}): RelayAccessGrantDto => ({
  id, ownerUserId: 'me', ownerUsername: 'du', targetUserId: 'u-joe', targetUsername: 'joe',
  deviceId: 'dev-1', deviceName: 'Supervisor', deviceConnected: true, scope, threadId: null, threadTitle: null,
  workspaceId: scope === 'workspace' ? 'ws-2' : null, workspaceLabel: scope === 'workspace' ? 'ElAgente' : null,
  workspaceScope: 'all', workspaceIds: [], label: null, threadAccess: 'control', workspaceAccess: 'none', canCreateThreads: false,
  createdAt: '2026-10-02T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [],
  ...over,
});
const portal = (over: Partial<RelayPortalSummaryDto>): RelayPortalSummaryDto => ({
  user: { id: 'me' } as RelayPortalSummaryDto['user'], devices: [], sharedWithMe: [], sharedByMe: [], ...over,
});

describe('shared items', () => {
  it('puts thread shares and every grant scope of one direction in one list, most recent first', () => {
    const items = sharedItems(portal({
      sharedByMe: [share('a', { lastAccessedAt: '2026-10-20T00:00:00Z' })],
      grantsByMe: [grant('d', 'device'), grant('w', 'workspace', { targetUserId: 'u-void', targetUsername: 'void111' })],
      sharedWithMe: [share('in', { ownerUsername: 'alwyn', ownerUserId: 'u-al' })],
    }), 'out');
    expect(items.map(item => [item.key, item.scope, item.title, item.personName])).toEqual([
      ['share:a', 'thread', 'Thread a', 'joe'],
      // Equally recent shares read by name.
      ['grant:w', 'workspace', 'ElAgente', 'void111'],
      ['grant:d', 'device', 'Supervisor', 'joe'],
    ]);
    const incoming = sharedItems(portal({
      sharedWithMe: [share('in', { ownerUsername: 'alwyn', ownerUserId: 'u-al' })],
      sharedThreadsWithMe: [grant('ws', 'workspace', { ownerUsername: 'alwyn', ownerUserId: 'u-al' })],
      sharedDevicesWithMe: [grant('dev', 'device', { ownerUsername: 'qing', ownerUserId: 'u-q', deviceId: 'dev-2', deviceName: 'gpu-lab' })],
    }), 'in');
    expect(incoming.map(item => [item.scope, item.personName])).toEqual([['workspace', 'alwyn'], ['device', 'qing'], ['thread', 'alwyn']]);
  });

  it('reads thread shares once although the relay repeats them among the grants shared with me', () => {
    const repeated = share('in', { ownerUsername: 'alwyn', ownerUserId: 'u-al' });
    const items = sharedItems(portal({
      sharedWithMe: [repeated],
      // The relay's sharedThreadsWithMe is sharedWithMe plus workspace and thread grants.
      sharedThreadsWithMe: [repeated as unknown as RelayAccessGrantDto, grant('ws', 'workspace', { ownerUsername: 'alwyn', ownerUserId: 'u-al' })],
    }), 'in');
    expect(items.map(item => [item.key, item.scope])).toEqual([['grant:ws', 'workspace'], ['share:in', 'thread']]);
  });

  it('filters by kind, attention and a search over names, devices and people', () => {
    const items = sharedItems(portal({ sharedByMe: [share('a')], grantsByMe: [grant('w', 'workspace', { targetUsername: 'void111' })] }), 'out');
    expect(filterItems(items, 'workspace', '', new Map()).map(item => item.key)).toEqual(['grant:w']);
    expect(filterItems(items, 'attention', '', new Map([['share:a', 'unverified']])).map(item => item.key)).toEqual(['share:a']);
    expect(filterItems(items, 'all', 'VOID', new Map()).map(item => item.key)).toEqual(['grant:w']);
    expect(filterItems(items, 'all', 'creativo', new Map()).map(item => item.key)).toEqual(['share:a']);
  });

  it('groups a device as whole device, then workspaces with the threads shared in them', () => {
    const items = sharedItems(portal({
      sharedByMe: [share('a'), share('b', { workspaceId: 'ws-2', workspaceLabel: 'ElAgente' })],
      grantsByMe: [grant('w', 'workspace'), grant('d', 'device')],
    }), 'out');
    const [device] = groupByDevice(items);
    expect(device!.rows.map(row => row.type === 'item' ? `${row.depth}:${row.item.key}` : `label:${row.label}`)).toEqual([
      '0:grant:d', 'label:creativocli', '1:share:a', '0:grant:w', '1:share:b',
    ]);
  });

  it('groups by the other person with counts per kind', () => {
    const items = sharedItems(portal({
      sharedByMe: [share('a')],
      grantsByMe: [grant('d', 'device'), grant('w', 'workspace', { targetUserId: 'u-void', targetUsername: 'void111' })],
    }), 'out');
    expect(groupByPerson(items).map(person => [person.personName, person.counts])).toEqual([
      ['joe', { device: 1, workspace: 0, thread: 1 }],
      ['void111', { device: 0, workspace: 1, thread: 0 }],
    ]);
  });
});
