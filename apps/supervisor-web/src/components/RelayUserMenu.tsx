import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useMenuKeyboard } from '@pockymoe/thread-ui';
import { CircleHelp, LogOut, Settings, UserRound, MonitorSmartphone } from 'lucide-react';
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import { Link, useLocation, useNavigate } from 'react-router-dom';

import { useAppShellNav } from './AppShellNavContext';
import { useTour } from './onboarding/TourProvider';
import type { RelaySessionDto } from '@pockymoe/shared';
import { popoverPosition } from '../lib/popoverPosition';
import {
  ApiError,
  fetchRelaySession,
  relayLogout,
  relayModeActive,
} from '../lib/api';

function initials(username: string | null | undefined) {
  const normalized = username?.trim() ?? '';
  if (!normalized) {
    return '??';
  }
  return Array.from(normalized).slice(0, 2).join('').toUpperCase();
}

function menuErrorMessage(caught: unknown) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : translate("auth.unableToLogOutTryAgain");
}

export function RelayUserMenu({
  className = '',
  menuAlign = 'left',
}: {
  className?: string;
  menuAlign?: 'left' | 'right';
}) {
  useI18n();
  const shellNav = useAppShellNav();
  const tour = useTour();
  const navigate = useNavigate();
  const location = useLocation();
  const menuId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const initialFocusRef = useRef<'first' | 'last'>('first');
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 8, top: 8, maxWidth: 256, maxHeight: 600 });
  const [loggingOut, setLoggingOut] = useState(false);
  const [logoutError, setLogoutError] = useState<string | null>(null);

  useEffect(() => {
    if (!relayModeActive()) {
      return;
    }
    let cancelled = false;
    const refresh = () => { fetchRelaySession()
      .then((nextSession) => {
        if (!cancelled) {
          setSession(nextSession.authenticated ? nextSession : null);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSession(null);
        }
      }); };
    refresh();
    window.addEventListener('remote-codex-account-updated', refresh);
    return () => {
      cancelled = true;
      window.removeEventListener('remote-codex-account-updated', refresh);
    };
  }, [location.pathname]);

  useEffect(() => {
    setOpen(false);
    setLogoutError(null);
  }, [location.pathname]);

  useLayoutEffect(() => {
    if (!open) return;
    function placeMenu() {
      const anchor = triggerRef.current?.getBoundingClientRect();
      const menu = menuRef.current;
      const viewport = window.visualViewport;
      const view = {
        width: viewport?.width ?? window.innerWidth,
        height: viewport?.height ?? window.innerHeight,
        left: viewport?.offsetLeft ?? 0,
        top: viewport?.offsetTop ?? 0,
      };
      if (!anchor || !menu
        || anchor.bottom <= view.top || anchor.top >= view.top + view.height
        || anchor.right <= view.left || anchor.left >= view.left + view.width) {
        setOpen(false);
        return;
      }
      // Browser chrome can resize/offset the visual viewport without moving the
      // document. Share the same viewport coordinates as other anchored panels.
      setPosition(popoverPosition(anchor, {
        width: 256,
        height: menu.scrollHeight + menu.offsetHeight - menu.clientHeight,
      }, view, 8, 8, menuAlign === 'left' ? 'start' : 'end'));
    }
    const scroll = (event: Event) => {
      if (!menuRef.current?.contains(event.target as Node)) placeMenu();
    };
    placeMenu();
    window.addEventListener('resize', placeMenu);
    window.addEventListener('scroll', scroll, true);
    window.visualViewport?.addEventListener('resize', placeMenu);
    window.visualViewport?.addEventListener('scroll', placeMenu);
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(placeMenu);
    if (menuRef.current) observer?.observe(menuRef.current);
    return () => {
      window.removeEventListener('resize', placeMenu);
      window.removeEventListener('scroll', scroll, true);
      window.visualViewport?.removeEventListener('resize', placeMenu);
      window.visualViewport?.removeEventListener('scroll', placeMenu);
      observer?.disconnect();
    };
  }, [open, menuAlign]);

  function closeMenu(restoreFocus = true) {
    setOpen(false);
    if (restoreFocus) triggerRef.current?.focus();
  }

  useMenuKeyboard(menuRef, open, closeMenu);

  useEffect(() => {
    if (!open) {
      return;
    }

    if (initialFocusRef.current === 'last') menuItems(menuRef.current).at(-1)?.focus();
    initialFocusRef.current = 'first';

    function handlePointerDown(event: PointerEvent) {
      if (event.target instanceof Node
        && !triggerRef.current?.contains(event.target)
        && !menuRef.current?.contains(event.target)) {
        setOpen(false);
      }
    }

    document.addEventListener('pointerdown', handlePointerDown);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
    };
  }, [open]);

  const user = session?.user ?? null;
  const label = useMemo(() => initials(user?.username), [user?.username]);

  if (!relayModeActive() || !user) {
    return null;
  }

  async function logout() {
    if (loggingOut) {
      return;
    }
    setLoggingOut(true);
    setLogoutError(null);
    try {
      await relayLogout();
      setSession(null);
      navigate('/relay-portal');
    } catch (caught) {
      setLogoutError(menuErrorMessage(caught));
      setLoggingOut(false);
    }
  }

  return (
    <div
      className={`inline-flex shrink-0 ${className}`.trim()}
    >
      <button
        aria-controls={open ? menuId : undefined}
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={translate("auth.relayAccountMenuFor", { value1: user.username })}
        className="inline-flex h-11 w-11 items-center justify-center rounded-full border border-[var(--theme-border)] bg-[var(--theme-surface-strong)] text-sm font-semibold text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
        onClick={() => {
          initialFocusRef.current = 'first';
          setLogoutError(null);
          setOpen((current) => !current);
        }}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') {
            return;
          }
          event.preventDefault();
          initialFocusRef.current = event.key === 'ArrowUp' ? 'last' : 'first';
          setOpen(true);
        }}
        ref={triggerRef}
        type="button"
      >
        {user.avatarUrl ? <img src={user.avatarUrl} alt="" className="h-full w-full rounded-full object-cover" /> : label}
      </button>
      {/* Keep the whole menu's hit area outside the fixed, backdrop-filtered topbar. */}
      {open ? createPortal(
        <div
          className="fixed z-[100] w-64 max-w-[calc(100vw-1rem)] overflow-y-auto rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-[var(--theme-shadow)]"
          style={position}
          id={menuId}
          ref={menuRef}
          role="menu"
        >
          <div className="border-b border-[var(--theme-border)] px-3 py-2" role="presentation">
            <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
              {user.username}
            </p>
            <p className="truncate text-xs text-[var(--theme-fg-muted)]">{user.email}</p>
          </div>
          <Link
            className="flex h-11 items-center gap-2 rounded-md px-3 text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:bg-[var(--theme-hover)] focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
            onClick={() => closeMenu()}
            role="menuitem"
            to="/relay-account"
          >
            <UserRound aria-hidden="true" className="h-4 w-4" />
            {translate("auth.accountSettings")}</Link>

          {shellNav && <button type="button" role="menuitem"
            className="flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]"
            onClick={() => { closeMenu(); shellNav.openSettings(); }}>
            <Settings aria-hidden="true" className="h-4 w-4" />{translate('files.settings')}
          </button>}
          <Link to="/relay-devices" role="menuitem" onClick={() => closeMenu()}
            className="flex h-11 items-center gap-2 rounded-md px-3 text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]">
            <MonitorSmartphone aria-hidden="true" className="h-4 w-4" />{translate('files.deviceManagement')}
          </Link>
          {tour && <button type="button" role="menuitem"
            className="flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]"
            onClick={() => { closeMenu(); tour.openHub(); }}>
            <CircleHelp aria-hidden="true" className="h-4 w-4" />{translate('tour.entry')}
          </button>}

          <button
            aria-busy={loggingOut}
            aria-disabled={loggingOut}
            className="mt-2 flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--status-danger-fg)] transition hover:bg-[var(--status-danger-bg)] focus-visible:outline-none focus-visible:bg-[var(--status-danger-bg)] focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] aria-disabled:cursor-wait aria-disabled:opacity-60"
            onClick={() => void logout()}
            role="menuitem"
            type="button"
          >
            <LogOut aria-hidden="true" className="h-4 w-4" />
            {loggingOut ? translate("auth.loggingOut") : translate("auth.logOut")}
          </button>
          {logoutError ? (
            <p
              aria-live="assertive"
              className="mx-2 mb-2 rounded-md bg-[var(--status-danger-bg)] px-2 py-1.5 text-xs leading-5 text-[var(--status-danger-fg)]"
              role="alert"
            >
              {logoutError}
            </p>
          ) : null}
        </div>, document.body,
      ) : null}
    </div>
  );
}

function menuItems(menu: HTMLDivElement | null) {
  return Array.from(
    menu?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)') ?? [],
  );
}
