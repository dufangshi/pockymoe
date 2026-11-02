import { useEffect, useRef, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { useThemeRegistry } from '../lib/themePacks';
import { useFontSize } from '../lib/fontSize';
import { fetchRelaySession, relayModeActive } from '../lib/api';
import { DesktopIndexStore } from '../desktop/data';
import { focusedKeyFromPath, handleHostCommand, syncActivity } from '../desktop/integration';
import { mobileHost, playHaptic, useHostSurface } from '../desktop/surface';
import { handleMobileBack } from './back';
import { setLiveActivityToken, restoreMobilePush, registerMobilePush, clearMobilePush } from './push';
import { loadPushSettings } from '../lib/relayPush';

export function MobileHostBoundary({ children, effectiveTheme }: { children: ReactNode; effectiveTheme: 'light' | 'dark' }) {
  const navigate = useNavigate(); const { locale } = useI18n(); const registry = useThemeRegistry(); const [fontSize] = useFontSize();
  useEffect(() => {
    const host = mobileHost(); if (!host) return;
    return host.onCommand(command => {
      if (command.type === 'back') host.commandResult(command.id, { ok: true, data: { handled: handleMobileBack(navigate) } });
      else if (command.type === 'liveActivityToken') void setLiveActivityToken(command.token).then(() => host.commandResult(command.id, { ok: true }), () => host.commandResult(command.id, { ok: false }));
    });
  }, [navigate]);
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const surface = Array.from(document.querySelectorAll<HTMLElement>('.thread-ui-shell, .auth-shell')).find(node => node.getClientRects().length) ?? document.documentElement;
      const style = getComputedStyle(surface);
      mobileHost()?.appearance.sync({ effective: effectiveTheme, pack: registry.applied, accent: style.getPropertyValue('--theme-accent-solid').trim(), background: style.getPropertyValue('--app-bg').trim() || style.backgroundColor, fontScale: fontSize / 16, locale });
    });
    return () => cancelAnimationFrame(frame);
  }, [effectiveTheme, registry.applied, fontSize, locale]);
  useEffect(() => {
    let previous = '';
    const title = () => { if (document.title !== previous) { previous = document.title; mobileHost()?.window.setTitle(previous); } };
    title(); const observer = new MutationObserver(title); observer.observe(document.head, { childList: true, subtree: true, characterData: true });
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    const logout = () => clearMobilePush();
    window.addEventListener('pockymoe-account-logout', logout);
    const warning = (event: MouseEvent) => {
      if (event.target instanceof Element && event.target.closest('[role="dialog"] .ui-action-danger:not(:disabled), [role="alertdialog"] .ui-action-danger:not(:disabled)')) playHaptic('warning');
    };
    document.addEventListener('click', warning, true); return () => { document.removeEventListener('click', warning, true); window.removeEventListener('pockymoe-account-logout', logout); };
  }, []);
  return children;
}

/** Account activity stays alive across device routes without desktop navigation. */
export function MobileActivity({ children }: { children: ReactNode }) {
  const surface = useHostSurface(); const navigate = useNavigate(); const location = useLocation();
  const route = useRef(location.pathname); route.current = location.pathname;
  useEffect(() => { if (surface === 'mobile') window.dispatchEvent(new Event('pockymoe-desktop-route')); }, [surface, location.pathname]);
  useEffect(() => {
    const host = mobileHost(); if (!host || !relayModeActive()) return;
    let cancelled = false; let store: DesktopIndexStore | undefined; let stop = () => {}; let commands = () => {};
    void (async () => {
      const session = await fetchRelaySession(); if (cancelled || !session.authenticated || !session.user) return;
      void loadPushSettings().then(settings => {
        if (!cancelled && restoreMobilePush(session.user!.id, settings)) return registerMobilePush();
      }).catch(() => undefined);
      store = new DesktopIndexStore(session.user.id); await store.hydrate(); if (cancelled) return;
      store.start(); stop = syncActivity(store, host, () => focusedKeyFromPath(route.current), true);
      const index = store;
      commands = host.onCommand(command => {
        if (command.type === 'back' || command.type === 'liveActivityToken') return;
        void handleHostCommand(command, { store: index, navigate, openMap: () => navigate('/'), revealInMap: () => navigate('/'), newThread: () => navigate('/relay-devices'), openUpstreams: () => navigate('/relay-settings?tab=connections') }).then(result => host.commandResult(command.id, result));
      });
    })().catch(() => { /* Device/index retries are owned by the shared store. */ });
    return () => { cancelled = true; stop(); commands(); store?.stop(); };
  }, [surface, navigate]);
  return children;
}
