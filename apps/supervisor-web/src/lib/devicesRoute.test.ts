import { afterEach, expect, it, vi } from 'vitest';
const host = vi.hoisted(() => ({ mobile: false, desktop: false }));
vi.mock('../desktop/surface', () => ({ hostSurface: () => host.desktop ? 'desktop' : host.mobile ? 'mobile' : 'browser' }));
afterEach(() => vi.unstubAllGlobals());
it.each([[false, false, '/desktop/devices'], [false, true, '/relay-devices'], [true, false, '/relay-devices'], [true, true, '/relay-devices']])('chooses once on load: mobile=%s phone=%s', async (mobile, phone, expected) => {
  vi.resetModules(); host.mobile = mobile; host.desktop = false;
  vi.stubGlobal('matchMedia', () => ({ matches: phone }));
  const { devicesHref } = await import('./devicesRoute');
  expect(devicesHref('?tab=shared&dir=out&shareDevice=d&action=share')).toBe(expected + '?tab=shared&dir=out&shareDevice=d&action=share');
  vi.stubGlobal('matchMedia', () => ({ matches: !phone }));
  expect(devicesHref()).toBe(expected);
});

it('keeps the desktop client on its Devices page even at phone width', async () => {
  vi.resetModules(); host.mobile = false; host.desktop = true;
  vi.stubGlobal('matchMedia', () => ({ matches: true }));
  const { devicesHref } = await import('./devicesRoute');
  expect(devicesHref()).toBe('/desktop/devices');
});
