import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { useThemeRegistry } from '../lib/themePacks';
import { useFontSize } from '../lib/fontSize';
import { desktopHost, HOST_BRIDGE_VERSION } from './host';
import './window-frame.css';

/** Account-independent window chrome: authentication pages need the same drag
 * region and appearance channel as the signed-in workbench. */
export default function DesktopWindowFrame({ children, effectiveTheme }: { children: ReactNode; effectiveTheme: 'light' | 'dark' }) {
  const root = useRef<HTMLDivElement>(null), page = useRef<HTMLDivElement>(null), host = desktopHost();
  const [thumb, setThumb] = useState({ height: 0, top: 0 });
  const measureFrame = useRef<number | null>(null);
  const measure = () => {
    if (measureFrame.current !== null) return;
    measureFrame.current = requestAnimationFrame(() => {
      measureFrame.current = null; const el = page.current; if (!el) return;
      const height = el.scrollHeight > el.clientHeight ? Math.max(28, el.clientHeight ** 2 / el.scrollHeight) : 0;
      const top = height ? el.scrollTop / Math.max(1, el.scrollHeight - el.clientHeight) * (el.clientHeight - height) : 0;
      setThumb(previous => previous.height === height && previous.top === top ? previous : { height, top });
    });
  };
  useEffect(() => {
    const el = page.current; if (!el) return;
    const resize = new ResizeObserver(measure); resize.observe(el);
    const changes = new MutationObserver(measure); changes.observe(el, { childList: true, subtree: true });
    measure(); window.addEventListener('resize', measure);
    return () => { resize.disconnect(); changes.disconnect(); window.removeEventListener('resize', measure); if (measureFrame.current !== null) cancelAnimationFrame(measureFrame.current); };
  }, []);
  const { locale } = useI18n(); const registry = useThemeRegistry(); const [fontSize] = useFontSize();
  useEffect(() => {
    let current = true;
    void host?.capabilities().then(capabilities => {
      if (!current || !root.current || capabilities.bridgeVersion !== HOST_BRIDGE_VERSION) return;
      const chrome = capabilities.windowChrome;
      root.current.style.setProperty('--desktop-controls-left', chrome.controlsSide === 'left' ? `${chrome.trafficLightsWidth}px` : '0px');
      root.current.style.setProperty('--desktop-controls-right', chrome.controlsSide === 'right' ? `${chrome.trafficLightsWidth}px` : '0px');
    }).catch(() => undefined);
    return () => { current = false; };
  }, [host]);
  useEffect(() => {
    if (typeof host?.appearance?.sync !== 'function') return;
    const frame = requestAnimationFrame(() => {
      const surface = Array.from(document.querySelectorAll<HTMLElement>('.thread-ui-shell')).find(node => node.getClientRects().length) ?? root.current;
      if (!surface) return;
      const accent = getComputedStyle(surface).getPropertyValue('--theme-accent-solid').trim();
      host.appearance.sync({ effective: effectiveTheme, pack: registry.applied, accent, fontScale: fontSize / 16, locale });
    });
    return () => cancelAnimationFrame(frame);
  }, [host, effectiveTheme, registry.applied, fontSize, locale]);
  return <div ref={root} className="desktop-window-frame">
    <div className="desktop-window-titlebar" data-desktop-titlebar aria-hidden="true" />
    <div ref={page} className="desktop-window-page" onScroll={measure}>{children}</div>
    {thumb.height > 0 && <div className="desktop-window-scroll-track" aria-hidden="true"><div className="desktop-window-scroll-thumb" style={{ height: thumb.height, transform: `translateY(${thumb.top}px)` }} onPointerDown={event => {
      event.preventDefault(); const el = page.current!; const start = event.clientY, at = el.scrollTop;
      const target = event.currentTarget; target.setPointerCapture(event.pointerId);
      const move = (next: PointerEvent) => { el.scrollTop = at + (next.clientY - start) * (el.scrollHeight - el.clientHeight) / Math.max(1, el.clientHeight - thumb.height); };
      const end = () => { target.removeEventListener('pointermove', move); target.removeEventListener('pointerup', end); target.removeEventListener('pointercancel', end); };
      target.addEventListener('pointermove', move); target.addEventListener('pointerup', end); target.addEventListener('pointercancel', end);
    }} /></div>}
  </div>;
}
