// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { handleMobileBack } from './back';
afterEach(() => { document.body.innerHTML = ''; history.replaceState(null, ''); vi.restoreAllMocks(); });
describe('mobile back command', () => {
  function overlay(role: string) {
    const node = document.createElement('div'); node.setAttribute('role', role); node.innerHTML = '<button>Close</button>';
    node.getClientRects = () => [{}] as unknown as DOMRectList;
    document.body.append(node); return node;
  }
  it('closes only the last portal before navigating, even with focus below it', () => {
    const navigate = vi.fn(); const sheet = overlay('dialog'); const menu = overlay('menu');
    sheet.querySelector('button')!.focus();
    const closeSheet = vi.fn(); sheet.addEventListener('keydown', closeSheet);
    menu.addEventListener('keydown', event => { expect(event.key).toBe('Escape'); menu.remove(); });
    history.replaceState({ idx: 2 }, '');
    expect(handleMobileBack(navigate)).toBe(true); expect(menu.isConnected).toBe(false);
    expect(closeSheet).not.toHaveBeenCalled(); expect(navigate).not.toHaveBeenCalled();
  });
  it('consumes back on a guarded sheet and leaves its close policy in control', () => {
    const sheet = overlay('dialog'); const navigate = vi.fn();
    expect(handleMobileBack(navigate)).toBe(true); expect(sheet.isConnected).toBe(true); expect(navigate).not.toHaveBeenCalled();
  });
  it('closes the settings sheet opened by a direct settings route', () => {
    const sheet = overlay('dialog'); sheet.className = 'thread-graph-settings-dialog';
    const close = vi.fn(); sheet.addEventListener('keydown', close);
    expect(handleMobileBack(vi.fn())).toBe(true);
    expect(close).toHaveBeenCalledOnce();
  });
  it('uses only in-app history, including a cold deep-link root', () => {
    const navigate = vi.fn();
    expect(handleMobileBack(navigate)).toBe(false);
    history.replaceState({ idx: 1 }, ''); expect(handleMobileBack(navigate)).toBe(true); expect(navigate).toHaveBeenCalledWith(-1);
  });
});
