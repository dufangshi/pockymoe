import { act, render, screen } from '@testing-library/react';
import { Profiler } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DesktopIndexStore, DesktopStoreContext, useDesktopSelector } from './index';

vi.mock('../../lib/api', () => ({ connectSupervisorEvents: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn(), request: vi.fn() }));
let store: DesktopIndexStore;
beforeEach(() => {
  vi.useFakeTimers();
  store = new DesktopIndexStore('fixture');
});
afterEach(() => {
  store.stop(); delete document.documentElement.dataset.pockymoeRendering;
  delete (document as unknown as { hidden?: boolean }).hidden;
  vi.restoreAllMocks(); vi.useRealTimers();
});
function View({ enabled = true }: { enabled?: boolean }) {
  const keys = useDesktopSelector(index => index.recentThreadKeys, Object.is, enabled);
  return <output data-testid="keys">{keys?.[0]}</output>;
}
const view = (enabled = true, committed = () => {}) => <DesktopStoreContext.Provider value={store}>
  <Profiler id="index-view" onRender={committed}><View enabled={enabled} /></Profiler>
</DesktopStoreContext.Provider>;

it('commits a burst in one frame while native subscribers observe every ordered change', () => {
  const native: string[] = []; store.subscribe(() => native.push(store.getSnapshot().recentThreadKeys![0]!));
  const committed = vi.fn(); render(view(true, committed)); committed.mockClear();
  act(() => { for (let i = 0; i < 30; i++) store.rememberThread(`thread-${i}`); });
  expect(native).toHaveLength(30); expect(native[0]).toBe('thread-0'); expect(native.at(-1)).toBe('thread-29');
  expect(screen.getByTestId('keys')).toHaveTextContent(''); expect(committed).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(20));
  expect(screen.getByTestId('keys')).toHaveTextContent('thread-29'); expect(committed).toHaveBeenCalledOnce();
});

it('does not subscribe an offscreen retained view and catches up when it becomes active', () => {
  const committed = vi.fn(); const mounted = render(view(false, committed)); committed.mockClear();
  act(() => store.rememberThread('background'));
  act(() => vi.advanceTimersByTime(100));
  expect(committed).not.toHaveBeenCalled(); expect(screen.getByTestId('keys')).toHaveTextContent('');
  mounted.rerender(view(true, committed));
  expect(screen.getByTestId('keys')).toHaveTextContent('background');
});

it('shares a frame across multiple views of the same account', () => {
  const committed = vi.fn(), frames = vi.spyOn(window, 'requestAnimationFrame');
  render(<DesktopStoreContext.Provider value={store}><Profiler id="account" onRender={committed}><View /><View /></Profiler></DesktopStoreContext.Provider>);
  committed.mockClear(); frames.mockClear();
  act(() => { store.rememberThread('first'); store.rememberThread('latest'); });
  expect(frames).toHaveBeenCalledOnce();
  act(() => vi.advanceTimersByTime(20));
  expect(screen.getAllByTestId('keys').every(output => output.textContent === 'latest')).toBe(true);
  expect(committed).toHaveBeenCalledOnce();
});

it.each(['browser', 'desktop'])('cancels an outstanding frame and commits no %s-hidden updates', source => {
  const committed = vi.fn(); render(view(true, committed)); committed.mockClear();
  act(() => store.rememberThread('pending'));
  act(() => {
    if (source === 'browser') { Object.defineProperty(document, 'hidden', { configurable: true, value: true }); document.dispatchEvent(new Event('visibilitychange')); }
    else { document.documentElement.dataset.pockymoeRendering = 'paused'; window.dispatchEvent(new Event('pockymoe-rendering-change')); }
    store.rememberThread('latest');
  });
  act(() => vi.advanceTimersByTime(60_000));
  expect(committed).not.toHaveBeenCalled(); expect(screen.getByTestId('keys')).toHaveTextContent('');
  act(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, value: false }); delete document.documentElement.dataset.pockymoeRendering;
    document.dispatchEvent(new Event('visibilitychange')); window.dispatchEvent(new Event('pockymoe-rendering-change'));
  });
  expect(screen.getByTestId('keys')).toHaveTextContent('latest'); expect(committed).toHaveBeenCalledOnce();
});
