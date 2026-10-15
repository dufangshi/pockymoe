import { HostFeatureGate, useHostScope } from '../lib/hostCapabilities';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef } from 'react';
import { CircleHelp, MonitorSmartphone, Menu, Settings, X } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router-dom';

import { useAppShellNav } from './AppShellNavContext';
import { menuItemClassName } from './appShellNavigationModel';
import { relayModeActive } from '../lib/api';
import { useTour } from './onboarding/TourProvider';
export { AppShellSettingsDialog } from './AppShellSettingsDialog';

export function AppShellMenuButton({ className = '' }: { className?: string }) {
  const { locale: i18nLocale } = useI18n();
  const shellNav = useAppShellNav();
  const hostScope = useHostScope();

  if (!shellNav || hostScope) {
    return null;
  }

  return (
    <button
      type="button"
      aria-label={shellNav.navOpen ? translate("files.closeNavigation") : translate("files.openNavigation")}
      aria-expanded={shellNav.navOpen}
      aria-controls="app-shell-navigation-menu"
      onClick={shellNav.toggleNav}
      className={`product-icon-button text-[var(--theme-fg)] ${className}`.trim()}
    >
      {shellNav.navOpen ? (
        <X data-icon="close" aria-hidden="true" className="h-4 w-4" />
      ) : (
        <Menu data-icon="menu" aria-hidden="true" className="h-4 w-4" />
      )}
    </button>
  );
}

export function AppShellNavigationMenu({
  className = '',
}: {
  className?: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const shellNav = useAppShellNav();
  const hostScope = useHostScope();
  const location = useLocation();
  const navigate = useNavigate();
  const menuRef = useRef<HTMLDivElement | null>(null);
  const tour = useTour();
  useEffect(() => {
    if (!shellNav?.navOpen) {
      return;
    }

    const activeNav = shellNav;

    function handlePointerDown(event: PointerEvent) {
      const target = event.target as Node | null;
      if (!target) {
        return;
      }

      const menuNode = menuRef.current;
      if (menuNode?.contains(target)) {
        return;
      }

      const trigger =
        target instanceof Element
          ? target.closest('[aria-controls="app-shell-navigation-menu"]')
          : null;
      if (trigger) {
        return;
      }

      activeNav.closeNav();
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        activeNav.closeNav();
        document
          .querySelector<HTMLElement>(
            '[aria-controls="app-shell-navigation-menu"]',
          )
          ?.focus();
      }
    }

    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    const focusTimer = window.setTimeout(() => {
      menuRef.current?.querySelector<HTMLElement>('button, a[href]')?.focus();
    }, 0);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [shellNav]);

  if (!shellNav?.navOpen || hostScope) {
    return null;
  }

  return (
    <div
      ref={menuRef}
      id="app-shell-navigation-menu"
      onPointerDown={(event) => {
        event.stopPropagation();
      }}
      onMouseDown={(event) => {
        event.stopPropagation();
      }}
      onTouchStart={(event) => {
        event.stopPropagation();
      }}
      className={`w-64 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2 shadow-[var(--theme-shadow)] ${className}`.trim()}
    >
      <nav
        aria-label={translate("files.supervisorNavigation")}
        className="flex flex-col gap-0.5 text-sm"
      >
        {relayModeActive() && (
          <HostFeatureGate feature="relay"><button
            type="button"
            onClick={() => {
              shellNav.closeNav();
              navigate('/relay-devices');
            }}
            className={menuItemClassName(
              location.pathname === '/relay-devices',
            )}
          >
            <MonitorSmartphone data-icon="devices" aria-hidden="true" className="h-4 w-4" />
            {translate("files.deviceManagement")}</button></HostFeatureGate>
        )}
        <button
          type="button"
          onClick={() => {
            shellNav.openSettings();
          }}
          className={menuItemClassName()}
        >
          <Settings data-icon="settings" aria-hidden="true" className="h-4 w-4" />
          {translate("files.settings")}</button>
        {tour && (
          <button
            type="button"
            onClick={() => {
              shellNav.closeNav();
              tour.openHub();
            }}
            className={menuItemClassName()}
          >
            <CircleHelp data-icon="help" aria-hidden="true" className="h-4 w-4" />
            {translate('tour.entry')}</button>
        )}
      </nav>
    </div>
  );
}
