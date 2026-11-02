// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { playHaptic, useHostSurface } from './surface';
import type { PockymoeHost } from './host';
afterEach(() => { cleanup(); delete window.pockymoeHost; });
function Surface() { return <span>{useHostSurface()}</span>; }
it('keeps browsers separate from mobile and desktop, awaiting capabilities', async () => {
  render(<Surface />); expect(screen.getByText('browser')).toBeTruthy(); cleanup();
  let resolve!: (value: unknown) => void;
  window.pockymoeHost = { capabilities: () => new Promise(r => { resolve = r; }) } as unknown as PockymoeHost;
  render(<Surface />); expect(screen.getByText('pending')).toBeTruthy();
  await act(async () => resolve({ surface: 'mobile', platform: 'ios', haptics: true }));
  expect(screen.getByText('mobile')).toBeTruthy();
});
it('plays only advertised mobile haptics and tolerates a failing native call', async () => {
  const play = vi.fn();
  window.pockymoeHost = { capabilities: async () => ({ surface: 'mobile', platform: 'android', haptics: true }), haptics: { play } } as unknown as PockymoeHost;
  render(<Surface />); await act(async () => {});
  playHaptic('sent'); expect(play).toHaveBeenCalledWith('sent');
  play.mockImplementation(() => { throw Error('system unavailable'); }); expect(() => playHaptic('warning')).not.toThrow();
  cleanup(); window.pockymoeHost = { capabilities: async () => ({ surface: 'desktop' }), haptics: { play } } as unknown as PockymoeHost;
  render(<Surface />); await act(async () => {}); play.mockClear(); playHaptic('attention'); expect(play).not.toHaveBeenCalled();
});
