import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { deviceActions } from './deviceActions';
import { workspaceActions } from './workspaceActions';

describe('device management stays in the client', () => {
  it('management components cannot call an external browser', () => {
    for (const file of ['../../desktop/devices/DeviceCard.tsx', '../../desktop/devices/index.tsx', '../../desktop/MapPage.tsx', '../DeviceManagement.tsx', './ManagedObjectMenu.tsx', './deviceActions.tsx', './workspaceActions.tsx', '../../pages/WorkspacesPage.tsx', '../../pages/RelayAccountPage.tsx', '../AccountSecurityPanel.tsx', '../SharingSettingsPanel.tsx', '../RelaySecurity.tsx']) {
      expect(readFileSync(new URL(file, import.meta.url), 'utf8'), file).not.toMatch(/\b(?:openExternal|window\.open)\s*\(/);
    }
  });
  it('opens and starts work by navigating locally, and manages in place without leaving the page', () => {
    const navigate = vi.fn(), manage = vi.fn(), reveal = vi.fn(), external = vi.fn();
    window.pockymoeHost = { shell: { openExternal: external } } as never;
    const items = deviceActions({ id: 'offline', name: 'Offline', connected: false }, { navigate, manage, reveal, renameSupported: false });
    expect(items.map(item => item.id)).toEqual(['open', 'new', 'details', 'reveal', 'rename', 'share', 'isolate', 'settings', 'remove']);
    for (const item of items) item.onSelect();
    expect(external).not.toHaveBeenCalled();
    // Only opening, a new thread and the device's page change the address; none carries ?action=.
    expect(navigate.mock.calls.map(([path]) => path)).toEqual(['/devices/offline/workspaces', '/devices/offline/threads/new', '/desktop/devices/offline']);
    expect(manage.mock.calls.map(([action]) => action)).toEqual(['rename', 'share', 'isolate', 'settings', 'remove']);
    expect(reveal).toHaveBeenCalledOnce();
    expect(items.filter(item => item.disabled).map(item => item.id)).toEqual(['new', 'rename']);
    expect(items.filter(item => item.disabled).every(item => item.disabledReason)).toBe(true);
    // An offline device still opens (rule 1): its workspaces page says it is offline.
    expect(items.find(item => item.id === 'open')?.disabled).toBeUndefined();
    expect(items.at(-1)).toMatchObject({ tone: 'danger', separator: true });
    delete window.pockymoeHost;
  });
  it('leaves out what a surface has no place for, keeping the order', () => {
    const items = deviceActions({ id: 'd', name: 'D', connected: true }, { navigate: vi.fn(), manage: vi.fn(), details: false });
    expect(items.map(item => item.id)).toEqual(['open', 'new', 'rename', 'share', 'isolate', 'settings', 'remove']);
    expect(items.some(item => item.disabled)).toBe(false);
  });
});

describe('devices and workspaces are managed alike', () => {
  const skeleton = (ids: string[]) => ids.filter(id => !['details', 'reveal'].includes(id)).map(id => id === 'settings' || id === 'path' ? 'own' : id);
  it('lists the same actions in the same order, with the same words', () => {
    setLocale('zh-CN', false);
    const device = deviceActions({ id: 'd', name: 'D', connected: true }, { navigate: vi.fn(), manage: vi.fn() });
    const workspace = workspaceActions({ agentIsolated: false }, { navigate: vi.fn(), manage: vi.fn(), openHref: '/threads?workspaceId=w', newThreadHref: '/threads/new?workspaceId=w', canShare: true });
    // Open / new thread, rename / share / isolate, the object's own entry (settings, or the path), then removal.
    expect(skeleton(workspace.map(item => item.id))).toEqual(skeleton(device.map(item => item.id)));
    expect(workspace.map(item => item.label)).toEqual(['打开', '新建线程', '重命名', '共享…', '隔离', '查看路径', '移除…']);
    expect(device.map(item => item.label)).toEqual(['打开', '新建线程', '设备详情', '重命名', '共享…', '隔离…', '设置…', '移除…']);
    for (const items of [device, workspace]) {
      expect(items.every(item => item.icon)).toBe(true);
      expect(items.filter(item => item.separator).map(item => item.id)).toEqual(['remove']);
      expect(items.filter(item => item.tone === 'danger').map(item => item.id)).toEqual(['remove']);
    }
    setLocale('en', false);
  });
  it('switches a workspace\'s isolation in place and offers none on a runtime without it', () => {
    setLocale('en', false);
    const navigate = vi.fn(), manage = vi.fn();
    const options = { navigate, manage, openHref: '/devices/d/threads?workspaceId=w', newThreadHref: '/devices/d/threads/new?workspaceId=w' };
    const isolated = workspaceActions({ agentIsolated: true }, options);
    expect(isolated.find(item => item.id === 'isolate')?.label).toBe('Stop isolating');
    // Not the owner through the relay: no sharing; an older runtime: no isolation.
    expect(isolated.some(item => item.id === 'share')).toBe(false);
    expect(workspaceActions({}, options).map(item => item.id)).toEqual(['open', 'new', 'rename', 'path', 'remove']);
    for (const item of isolated) item.onSelect();
    expect(navigate.mock.calls.map(([path]) => path)).toEqual(['/devices/d/threads?workspaceId=w', '/devices/d/threads/new?workspaceId=w']);
    expect(manage.mock.calls.map(([action]) => action)).toEqual(['rename', 'isolate', 'path', 'remove']);
  });
});
