import { expect, it } from 'vitest';
import { devicePageHref, devicesHref, deviceSettingsHref } from './devicesRoute';

it('sends every surface to the one device center, keeping old query state', () => {
  expect(devicesHref('?tab=shared&dir=out&shareDevice=d&action=share')).toBe('/desktop/devices?tab=shared&dir=out&shareDevice=d&action=share');
  expect(devicesHref()).toBe('/desktop/devices');
});

it('names a device page tab and a settings section in the address', () => {
  expect(devicePageHref('a b')).toBe('/desktop/devices/a%20b');
  expect(devicePageHref('d', 'access')).toBe('/desktop/devices/d?tab=access');
  expect(deviceSettingsHref('d')).toBe('/desktop/devices/d?tab=settings');
  expect(deviceSettingsHref('d', 'upstreams')).toBe('/desktop/devices/d?tab=settings&section=upstreams');
});
