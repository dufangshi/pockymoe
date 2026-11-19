import { afterEach, expect, it } from 'vitest';
import { chromeInsets } from './windowControls';

const windows = { kind: 'titleBarOverlay', trafficLightsWidth: 138, toolbarHeight: 38, controlsSide: 'right' } as const;
const mac = { kind: 'hiddenInset', trafficLightsWidth: 78, toolbarHeight: 38, controlsSide: 'left' } as const;
afterEach(() => { delete (navigator as { windowControlsOverlay?: unknown }).windowControlsOverlay; });

it('falls back to the reported device-independent sizes', () => {
  expect(chromeInsets(windows)).toEqual({ left: 0, right: 138, height: 38 });
  expect(chromeInsets(mac)).toEqual({ left: 78, right: 0, height: 38 });
});

it('follows the Window Controls Overlay geometry, which tracks page zoom', () => {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1125 });
  (navigator as { windowControlsOverlay?: unknown }).windowControlsOverlay = { visible: true, getTitlebarAreaRect: () => ({ x: 0, y: 0, width: 952.5, height: 47.5 }), addEventListener() {}, removeEventListener() {} };
  expect(chromeInsets(windows)).toEqual({ left: 0, right: 172.5, height: 47.5 });
});
