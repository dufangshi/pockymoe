/** Places that zoom what they show themselves (two-finger gestures of their own). */
const OWN_ZOOM = '.thread-graph-image-lightbox-viewport, .workspace-image-viewport, .thread-map-canvas';

/**
 * The page never zooms on a phone: Safari zoomed in when the prompt took focus
 * and left the reader to pinch it back. The viewport meta stops the focus zoom
 * and Android's pinch; Safari ignores `user-scalable=no`, so its pinch gestures
 * are refused here. Images and the thread map keep their own zoom.
 */
export function installPageZoomGuard(target: Document = document) {
  const ownsZoom = (event: Event) => event.target instanceof Element && Boolean(event.target.closest(OWN_ZOOM));
  const refuse = (event: Event) => { if (!ownsZoom(event)) event.preventDefault(); };
  // Safari's own pinch events.
  for (const type of ['gesturestart', 'gesturechange', 'gestureend']) target.addEventListener(type, refuse, { passive: false });
  // Any other browser's two-finger move that would pinch the page.
  target.addEventListener('touchmove', (event) => {
    if ((event as TouchEvent).touches.length > 1) refuse(event);
  }, { passive: false });
}
