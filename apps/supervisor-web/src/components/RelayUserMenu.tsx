import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { CircleHelp, LogOut, Settings, UserRound, MonitorSmartphone } from 'lucide-react';
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';

import { useAppShellNav } from './AppShellNavContext';
import { useTour } from './onboarding/TourProvider';
import type { RelaySessionDto } from '@pockymoe/shared';
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
  const wrapperRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const initialFocusRef = useRef<'first' | 'last'>('first');
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [open, setOpen] = useState(false);
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

  useEffect(() => {
    if (!open) {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      const items = menuItems(menuRef.current);
      const target = initialFocusRef.current === 'last' ? items.at(-1) : items[0];
      target?.focus();
      initialFocusRef.current = 'first';
    });

    function handlePointerDown(event: PointerEvent) {
      if (event.target instanceof Node && !wrapperRef.current?.contains(event.target)) {
        setOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== 'Escape') {
        return;
      }
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    }

    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
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

  function handleMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const items = menuItems(menuRef.current);
    if (items.length === 0) {
      return;
    }
    const currentIndex = items.indexOf(document.activeElement as HTMLElement);
    if (event.key === 'Home') {
      items[0]?.focus();
      return;
    }
    if (event.key === 'End') {
      items.at(-1)?.focus();
      return;
    }
    const direction = event.key === 'ArrowDown' ? 1 : -1;
    const nextIndex = currentIndex < 0
      ? direction > 0 ? 0 : items.length - 1
      : (currentIndex + direction + items.length) % items.length;
    items[nextIndex]?.focus();
  }

  return (
    <div
      className={`relative z-50 inline-flex shrink-0 ${className}`.trim()}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
      ref={wrapperRef}
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
      {open ? (
        <div
          className={`absolute top-full mt-2 w-64 overflow-hidden rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-[var(--theme-shadow)] ${
            menuAlign === 'right' ? 'right-0' : 'left-0'
          }`}
          id={menuId}
          onKeyDown={handleMenuKeyDown}
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
            onClick={() => setOpen(false)}
            role="menuitem"
            to="/relay-account"
          >
            <UserRound aria-hidden="true" className="h-4 w-4" />
            {translate("auth.accountSettings")}</Link>

          {shellNav && <button type="button" role="menuitem"
            className="flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]"
            onClick={() => { setOpen(false); shellNav.openSettings(); }}>
            <Settings aria-hidden="true" className="h-4 w-4" />{translate('files.settings')}
          </button>}
          <Link to="/relay-devices" role="menuitem" onClick={() => setOpen(false)}
            className="flex h-11 items-center gap-2 rounded-md px-3 text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]">
            <MonitorSmartphone aria-hidden="true" className="h-4 w-4" />{translate('files.deviceManagement')}
          </Link>
          {tour && <button type="button" role="menuitem"
            className="flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)]"
            onClick={() => { setOpen(false); tour.openHub(); }}>
            <CircleHelp aria-hidden="true" className="h-4 w-4" />{translate('tour.entry')}
          </button>}

          <button
            aria-busy={loggingOut}
            aria-disabled={loggingOut}
            className="flex h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--status-danger-fg)] transition hover:bg-[var(--status-danger-bg)] focus-visible:outline-none focus-visible:bg-[var(--status-danger-bg)] focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] aria-disabled:cursor-wait aria-disabled:opacity-60"
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
        </div>
      ) : null}
    </div>
  );
}

function menuItems(menu: HTMLDivElement | null) {
  return Array.from(
    menu?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)') ?? [],
  );
}
