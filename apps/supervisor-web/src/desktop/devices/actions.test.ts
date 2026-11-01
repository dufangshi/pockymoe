import { describe, expect, it, vi } from 'vitest';
import type { DesktopIndex } from '../data/types';
import { buildDeviceActions } from './actions';

const index: DesktopIndex = { accountId: 'a', devices: [{ id: 'device/a', name: 'Mac', connected: true, lastHeartbeatAt: null }, { id: 'offline', name: 'Server', connected: false, lastHeartbeatAt: null }], workspaces: [], threads: [], hydrated: true, refreshing: false, error: null };
describe('device palette actions', () => {
  it('routes management actions to explicit encoded devices and omits offline mutations', () => {
    const navigate = vi.fn();
    const actions = buildDeviceActions(index, { navigate });
    actions.find(a => a.id === 'device:device/a:update')!.run();
    expect(navigate).toHaveBeenCalledWith('/desktop/devices?device=device%2Fa&action=update');
    expect(actions.some(a => a.id === 'device:offline:new')).toBe(false);
    expect(actions.some(a => a.id === 'device:offline:update')).toBe(false);
    expect(actions.find(a => a.id === 'device:device/a:rename')?.label).toContain('Mac');
  });
  it('uses the navigation owner to reveal devices and create threads', () => {
    const onRevealDevice = vi.fn(), onNewThread = vi.fn();
    const actions = buildDeviceActions(index, { navigate: vi.fn(), onRevealDevice, onNewThread });
    actions.find(a => a.id === 'device:device/a:open')!.run(); actions.find(a => a.id === 'device:device/a:new')!.run();
    expect(onRevealDevice).toHaveBeenCalledWith('device/a'); expect(onNewThread).toHaveBeenCalledWith('device/a');
  });
});
