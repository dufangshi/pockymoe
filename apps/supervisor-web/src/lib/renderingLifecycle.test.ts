import { afterEach, expect, it, vi } from 'vitest';
import { subscribeWhenRendering } from './renderingLifecycle';

afterEach(() => { delete document.documentElement.dataset.pockymoeRendering; });
it('releases a rendering resource while paused and reinstalls it once on resume', () => {
  const release = vi.fn(); const install = vi.fn(() => release);
  const stop = subscribeWhenRendering(install, () => {});
  expect(install).toHaveBeenCalledOnce();
  document.documentElement.dataset.pockymoeRendering = 'paused'; window.dispatchEvent(new Event('pockymoe-rendering-change'));
  expect(release).toHaveBeenCalledOnce();
  delete document.documentElement.dataset.pockymoeRendering; window.dispatchEvent(new Event('pockymoe-rendering-change'));
  window.dispatchEvent(new Event('pockymoe-rendering-change'));
  expect(install).toHaveBeenCalledTimes(2);
  stop(); expect(release).toHaveBeenCalledTimes(2);
});
