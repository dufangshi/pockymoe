import { useEffect, useState } from 'react';
import type { DesktopCapabilities } from './host';

type Chrome = DesktopCapabilities['windowChrome'];
export interface ChromeInsets { left: number; right: number; height: number }
interface Overlay extends EventTarget { visible: boolean; getTitlebarAreaRect(): DOMRect }

/** Space the page leaves for the native window controls, in CSS pixels.
 * The host reports device-independent sizes, which stop matching once the page is
 * zoomed (Ctrl/Cmd +/-). The Window Controls Overlay geometry follows the zoom, so
 * it wins whenever the shell exposes it; the reported sizes are the fallback. */
export function chromeInsets(chrome: Chrome): ChromeInsets {
  const overlay = (navigator as Navigator & { windowControlsOverlay?: Overlay }).windowControlsOverlay;
  if (overlay?.visible) {
    const area = overlay.getTitlebarAreaRect();
    if (area.height > 0) return { left: Math.max(0, area.x), right: Math.max(0, window.innerWidth - area.x - area.width), height: area.height };
  }
  return { left: chrome.controlsSide === 'left' ? chrome.trafficLightsWidth : 0, right: chrome.controlsSide === 'right' ? chrome.trafficLightsWidth : 0, height: chrome.toolbarHeight };
}

export function insetStyle(insets: ChromeInsets): Record<string, string> {
  return { '--desktop-title-height': `${insets.height}px`, '--desktop-controls-left': `${insets.left}px`, '--desktop-controls-right': `${insets.right}px` };
}

/** Recomputes on overlay geometry changes (zoom, resize, maximize) and window resizes. */
export function useChromeInsets(chrome: Chrome | undefined): ChromeInsets | null {
  const [insets, setInsets] = useState<ChromeInsets | null>(() => chrome ? chromeInsets(chrome) : null);
  useEffect(() => {
    if (!chrome) { setInsets(null); return; }
    const update = () => setInsets(previous => {
      const next = chromeInsets(chrome);
      return previous && previous.left === next.left && previous.right === next.right && previous.height === next.height ? previous : next;
    });
    update();
    const overlay = (navigator as Navigator & { windowControlsOverlay?: Overlay }).windowControlsOverlay;
    overlay?.addEventListener('geometrychange', update); window.addEventListener('resize', update);
    return () => { overlay?.removeEventListener('geometrychange', update); window.removeEventListener('resize', update); };
  }, [chrome]);
  return insets;
}
