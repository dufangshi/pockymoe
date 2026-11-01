import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

if (!globalThis.URL.createObjectURL) {
  globalThis.URL.createObjectURL = (() => `blob:mock-${Math.random().toString(36).slice(2)}`) as typeof URL.createObjectURL;
}

if (!globalThis.URL.revokeObjectURL) {
  globalThis.URL.revokeObjectURL = (() => undefined) as typeof URL.revokeObjectURL;
}

if (typeof globalThis.window !== 'undefined' && !globalThis.window.matchMedia) {
  globalThis.window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
}

afterEach(() => {
  cleanup();
});

// jsdom has no layout. Give custom combobox triggers an on-screen rectangle so
// component tests can exercise real popup interactions rather than fake changes.
if (typeof HTMLElement !== 'undefined') {
  const measure = HTMLElement.prototype.getBoundingClientRect;
  HTMLElement.prototype.getBoundingClientRect = function () {
    const rect = measure.call(this);
    return this.getAttribute('role') === 'combobox' && rect.width === 0 && rect.height === 0
      ? DOMRect.fromRect({ x: 10, y: 10, width: 180, height: 32 }) : rect;
  };
}
