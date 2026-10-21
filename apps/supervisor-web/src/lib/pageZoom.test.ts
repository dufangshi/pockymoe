// @vitest-environment jsdom
import { beforeAll, expect, it } from 'vitest';
import { installPageZoomGuard } from './pageZoom';

beforeAll(() => installPageZoomGuard());

function fire(target: Element, type: string, touches = 1) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'touches', { value: Array.from({ length: touches }) });
  target.dispatchEvent(event);
  return event.defaultPrevented;
}

it('refuses page pinch gestures, Safari and others alike', () => {
  const page = document.createElement('div');
  document.body.append(page);
  expect(fire(page, 'gesturestart')).toBe(true);
  expect(fire(page, 'touchmove', 2)).toBe(true);
  // One finger still scrolls.
  expect(fire(page, 'touchmove', 1)).toBe(false);
});

it('leaves images and the thread map their own zoom', () => {
  for (const name of ['thread-graph-image-lightbox-viewport', 'workspace-image-viewport', 'thread-map-canvas']) {
    const viewer = document.createElement('div');
    viewer.className = name;
    const image = document.createElement('img');
    viewer.append(image);
    document.body.append(viewer);
    expect(fire(image, 'gesturestart')).toBe(false);
    expect(fire(image, 'touchmove', 2)).toBe(false);
  }
});
