import { beforeEach, expect, it, vi } from 'vitest';
import { installStaleBuildReload } from './staleBuildReload';

function fakeWindow() {
  const target = new EventTarget();
  const reload = vi.fn();
  return Object.assign(target, { sessionStorage: window.sessionStorage, location: { reload } }) as unknown as Window & { location: { reload: typeof reload } };
}
beforeEach(() => window.sessionStorage.clear());

it('reloads once when a deployment removed the chunk being loaded', () => {
  const win = fakeWindow();
  let clock = 1_000_000;
  installStaleBuildReload(win, () => clock);
  const first = new Event('vite:preloadError', { cancelable: true });
  win.dispatchEvent(first);
  expect(win.location.reload).toHaveBeenCalledOnce();
  expect(first.defaultPrevented).toBe(true);
  // Within a minute the error reaches the boundary instead of looping.
  clock += 30_000;
  const second = new Event('vite:preloadError', { cancelable: true });
  win.dispatchEvent(second);
  expect(win.location.reload).toHaveBeenCalledOnce();
  expect(second.defaultPrevented).toBe(false);
  clock += 60_000;
  win.dispatchEvent(new Event('vite:preloadError', { cancelable: true }));
  expect(win.location.reload).toHaveBeenCalledTimes(2);
});
