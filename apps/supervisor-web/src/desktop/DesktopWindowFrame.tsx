import { useEffect, useRef, type ReactNode } from 'react';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { useThemeRegistry } from '../lib/themePacks';
import { useFontSize } from '../lib/fontSize';
import { desktopHost, HOST_BRIDGE_VERSION } from './host';
import './window-frame.css';

/** Account-independent window chrome: authentication pages need the same drag
 * region and appearance channel as the signed-in workbench. */
export default function DesktopWindowFrame({ children, effectiveTheme }: { children: ReactNode; effectiveTheme: 'light' | 'dark' }) {
  const root = useRef<HTMLDivElement>(null), host = desktopHost();
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
    <div className="desktop-window-page">{children}</div>
  </div>;
}
