import { describe, expect, it, vi } from 'vitest';
import { deviceActions } from './deviceActions';

describe('device management stays in the client', () => {
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
