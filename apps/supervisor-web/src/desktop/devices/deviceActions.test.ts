import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { deviceActions } from './deviceActions';

describe('device management stays in the client', () => {
  it('management components cannot call an external browser', () => {
    for (const file of ['./DeviceCard.tsx', './index.tsx', '../MapPage.tsx', '../../components/DeviceManagement.tsx', '../../pages/RelayAccountPage.tsx', '../../components/AccountSecurityPanel.tsx', '../../components/SharingSettingsPanel.tsx', '../../components/RelaySecurity.tsx']) {
      expect(readFileSync(new URL(file, import.meta.url), 'utf8'), file).not.toMatch(/\b(?:openExternal|window\.open)\s*\(/);
    }
  });
  it('card and map actions navigate locally and keep unavailable actions in order', () => {
    const navigate = vi.fn(), reveal = vi.fn(), external = vi.fn();
    window.pockymoeHost = { shell: { openExternal: external } } as never;
    const items = deviceActions({ id: 'offline', name: 'Offline', connected: false } as never, false, navigate, reveal);
    expect(items.map(item => item.id)).toEqual(['reveal', 'details', 'workspaces', 'new', 'check', 'rename', 'share', 'isolate', 'remove']);
    for (const item of items) item.onSelect();
    expect(external).not.toHaveBeenCalled();
    expect(navigate.mock.calls.every(([path]) => path.startsWith('/') && !path.startsWith('//'))).toBe(true);
    expect(items.filter(item => item.disabled).every(item => item.disabledReason)).toBe(true);
    expect(items.find(item => item.id === 'workspaces')?.disabled).toBeUndefined();
    expect(items.at(-1)).toMatchObject({ tone: 'danger', separator: true });
    delete window.pockymoeHost;
  });
});
