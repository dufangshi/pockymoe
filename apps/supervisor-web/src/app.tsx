import { HostScopeRouteGate } from './components/HostScopeRouteGate';
import {
  AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY,
  readAutoCollapseCompletedTurns,
  TRANSCRIPT_PREFERENCE_EVENT,
} from './lib/transcriptPreferences';
import { appUrl, webBase } from './lib/webBase';
import { HostCapabilitiesProvider, UnscopedOnly, useHostScope } from './lib/hostCapabilities';
import { relayDeviceIdFromPath } from './lib/relayRoutes';
import { AccountSessionGate } from './components/AccountSessionGate';
import { SharedInvitePage } from './pages/SharedInvitePage';
import { WorkspaceDocumentNavigationGuard } from "./components/WorkspaceDocumentNavigationGuard";
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { installNotificationRouteResponder } from './lib/relayPush';
import { PortPreviewPage } from './pages/PortPreviewPage';
import { DeviceEnrollmentMonitor } from './components/DeviceEnrollmentMonitor';
import { AccountUnlockPrompt } from './components/AccountUnlockPrompt';
import { PublicThreadPage } from './pages/PublicThreadPage';
import {
  BrowserRouter,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
} from 'react-router-dom';
import { useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from 'react';
import { PluginProvider } from '@pockymoe/thread-ui';

import type {
  AgentBackendIdDto,
  AuthSessionDto,
  RelaySessionDto,
} from '../../../packages/shared/src/index';
import {
  defaultAgentBackendId,
  normalizeAgentBackendId,
} from '@pockymoe/shared';
import {
  AppShellNavContext,
  type PhoneHand,
  type ThemeMode,
} from './components/AppShellNavContext';
import {
  AppShellSettingsDialog,
} from './components/AppShellNavigation';
import { TourProvider } from './components/onboarding/TourProvider';
import { LoginPage } from './pages/LoginPage';
import { RelayAccountPage } from './pages/RelayAccountPage';
import { RelayAdminPage } from './pages/RelayAdminPage';
import { RelayDevicesPage } from './pages/RelayDevicesPage';
import { RelayGuidePage } from './pages/RelayGuidePage';
import { HelpPage } from './pages/HelpPage';
import { RelayHomePage } from './pages/RelayHomePage';
import { RelayPortalPage } from './pages/RelayPortalPage';
import { RelaySettingsPage } from './pages/RelaySettingsPage';
import { ThreadDetailPage } from './pages/ThreadDetailPage';
import { ThreadImportPage } from './pages/ThreadImportPage';
import { ThreadNewPage } from './pages/ThreadNewPage';
import { ThreadsPage } from './pages/ThreadsPage';
import { WorkspaceNewPage } from './pages/WorkspaceNewPage';
import { WorkspacesPage } from './pages/WorkspacesPage';
import {
  ApiError,
  deletePlugin,
  fetchAuthSession,
  fetchPlugins,
  fetchRelaySession,
  importPlugin,
  login,
  relayModeActive,
  updatePlugin,
} from './lib/api';
import { builtinFrontendPlugins } from './plugins/builtin-plugin-modules';
import { useComposerPreferences } from './lib/useComposerPreferences';
import { effectiveColorMode, themeChromeColor, useThemeRegistry } from './lib/themePacks';

const PHONE_DOCK_STORAGE_KEY = 'pockymoe.phoneDock';
const PHONE_HAND_STORAGE_KEY = 'pockymoe.phoneHand';

const THEME_STORAGE_KEY = 'remote-codex-theme-mode';
const BACKEND_STORAGE_KEY = 'remote-codex-default-backend';

function RoutePluginProvider({ children }: { children: ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  const adapter = useMemo(
    () => ({
      fetchPlugins,
      importPlugin,
      updatePlugin,
      deletePlugin,
    }),
    [],
  );

  return (
    <PluginProvider adapter={adapter} builtinPlugins={builtinFrontendPlugins}>
      {children}
    </PluginProvider>
  );
}

function readInitialThemeMode(): ThemeMode {
  if (typeof window === 'undefined') {
    return 'system';
  }

  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  if (stored === 'light' || stored === 'dark' || stored === 'system') {
    return stored;
  }

  return 'system';
}

function readInitialBackend(): AgentBackendIdDto {
  if (typeof window === 'undefined') {
    return defaultAgentBackendId;
  }

  const stored = window.localStorage.getItem(BACKEND_STORAGE_KEY);
  return normalizeAgentBackendId(stored) ?? defaultAgentBackendId;
}

function readInitialAutoCollapseCompletedTurns() {
  if (typeof window === 'undefined') {
    return true;
  }

  return readAutoCollapseCompletedTurns();
}

function formatDocumentTitle(pageTitle?: string | null) {
  const trimmed = pageTitle?.trim();
  return trimmed || 'Pockymoe';
}

function routeDocumentTitle(pathname: string) {
  if (pathname === '/' || pathname === '/relay-portal') {
    return translate("workbench.relayPortal");
  }
  if (pathname === '/relay-guide') {
    return translate("workbench.relaySetup");
  }
  if (pathname === '/help' || pathname.startsWith('/help/')) {
    return translate('common.help');
  }
  if (pathname === '/relay-admin') {
    return translate("workbench.relayAdmin");
  }
  if (pathname === '/relay-account') {
    return translate("workbench.account");
  }
  if (pathname === '/relay-devices') {
    return translate("workbench.devicesAndSharedSessions");
  }
  if (pathname === '/relay-settings') {
    return translate("workbench.settings");
  }
  if (
    pathname === '/workspaces' ||
    /^\/devices\/[^/]+\/workspaces$/.test(pathname)
  ) {
    return translate("workbench.workspaces");
  }
  if (
    pathname === '/workspaces/new' ||
    /^\/devices\/[^/]+\/workspaces\/new$/.test(pathname)
  ) {
    return translate("workbench.newWorkspace");
  }
  if (
    pathname === '/threads' ||
    /^\/devices\/[^/]+\/threads$/.test(pathname)
  ) {
    return translate("workbench.threads");
  }
  if (
    pathname === '/threads/import' ||
    /^\/devices\/[^/]+\/threads\/import$/.test(pathname)
  ) {
    return translate("workbench.importThread");
  }
  if (
    pathname === '/threads/new' ||
    /^\/devices\/[^/]+\/threads\/new$/.test(pathname)
  ) {
    return translate("workbench.newThread");
  }
  if (
    /^\/threads\/[^/]+$/.test(pathname) ||
    /^\/devices\/[^/]+\/threads\/[^/]+$/.test(pathname)
  ) {
    return translate("workbench.thread");
  }

  return null;
}

function DocumentTitleUpdater() {
  const { locale: i18nLocale } = useI18n();
  useEffect(installNotificationRouteResponder, []);
  useEffect(() => {
    if (!navigator.serviceWorker) return;
    const sync = () => navigator.serviceWorker.controller?.postMessage({ type: 'remote-codex-locale', locale: i18nLocale });
    sync();
    navigator.serviceWorker.addEventListener('controllerchange', sync);
    return () => navigator.serviceWorker.removeEventListener('controllerchange', sync);
  }, [i18nLocale]);
  const location = useLocation();

  useEffect(() => {
    document.title = formatDocumentTitle(routeDocumentTitle(location.pathname));
  }, [location.pathname, i18nLocale]);

  return null;
}

function systemThemePreference(): 'light' | 'dark' {
  if (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  ) {
    return 'dark';
  }

  return 'light';
}

function ScopedTourProvider({ children }: { children: ReactNode }) {
  return useHostScope() ? children : <TourProvider>{children}</TourProvider>;
}

function AppShell({
  themeMode,
  setThemeMode,
  effectiveTheme,
}: {
  themeMode: ThemeMode;
  setThemeMode: (mode: ThemeMode) => void;
  effectiveTheme: 'light' | 'dark';
}) {
  const { locale: i18nLocale } = useI18n();
  const [navOpen, setNavOpen] = useState(false);
  const composerPreferences = useComposerPreferences();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<string | null>(null);
  const [showReasoningSummaries, setShowReasoningSummaries] = useState(() => {
    try { return localStorage.getItem('remote-codex-show-reasoning-summaries') === 'true'; } catch { return false; }
  });
  const [defaultBackend, setDefaultBackendState] = useState<AgentBackendIdDto>(readInitialBackend);
  const [autoCollapseCompletedTurns, setAutoCollapseCompletedTurnsState] = useState(
    readInitialAutoCollapseCompletedTurns,
  );
  // One-handed phone layout, on unless turned off in Preferences.
  const [phoneDock, setPhoneDockState] = useState(() => {
    try { return localStorage.getItem(PHONE_DOCK_STORAGE_KEY) !== 'off'; } catch { return true; }
  });
  const [phoneHand, setPhoneHandState] = useState<PhoneHand>(() => {
    try { return localStorage.getItem(PHONE_HAND_STORAGE_KEY) === 'left' ? 'left' : 'right'; } catch { return 'right'; }
  });
  const location = useLocation();
  const isThreadUtilityRoute =
    /^\/threads\/(?:import|new)$/.test(location.pathname) ||
    /^\/devices\/[^/]+\/threads\/(?:import|new)$/.test(location.pathname);
  const isThreadDetailRoute =
    !isThreadUtilityRoute &&
    (/^\/threads\/[^/]+$/.test(location.pathname) ||
      /^\/devices\/[^/]+\/threads\/[^/]+$/.test(location.pathname));
  const isThreadsRoute =
    location.pathname === '/threads' ||
    /^\/devices\/[^/]+\/threads$/.test(location.pathname);
  const isViewportLockedRoute = isThreadDetailRoute || isThreadsRoute;
  const isThreadWorkspaceRoute =
    isThreadsRoute || isThreadDetailRoute;
  const isWorkspacesRoute =
    location.pathname === '/workspaces' ||
    /^\/devices\/[^/]+\/workspaces$/.test(location.pathname);
  const isSettingsRoute = location.pathname === '/relay-settings';
  const isNativeApp =
    new URLSearchParams(location.search).get('nativeApp') === '1';

  useEffect(() => {
    setNavOpen(false);
  }, [location.pathname, location.search]);

  function setDefaultBackend(backend: AgentBackendIdDto) {
    setDefaultBackendState(backend);
    window.localStorage.setItem(BACKEND_STORAGE_KEY, backend);
  }

  function setPhoneDock(enabled: boolean) {
    setPhoneDockState(enabled);
    try { localStorage.setItem(PHONE_DOCK_STORAGE_KEY, enabled ? 'on' : 'off'); } catch { /* Private browsing may disable storage. */ }
  }

  function setPhoneHand(hand: PhoneHand) {
    setPhoneHandState(hand);
    try { localStorage.setItem(PHONE_HAND_STORAGE_KEY, hand); } catch { /* Private browsing may disable storage. */ }
  }

  function setAutoCollapseCompletedTurns(enabled: boolean) {
    setAutoCollapseCompletedTurnsState(enabled);
    window.localStorage.setItem(
      AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY,
      enabled ? 'true' : 'false',
    );
    window.dispatchEvent(new Event(TRANSCRIPT_PREFERENCE_EVENT));
  }

  const shellNavValue = {
    ...composerPreferences,
    navOpen,
    openNav: () => setNavOpen(true),
    toggleNav: () => setNavOpen((current) => !current),
    closeNav: () => setNavOpen(false),
    settingsOpen,
    settingsSection,
    openSettings: () => {
      setNavOpen(false);
      setSettingsSection(null);
      setSettingsOpen(true);
    },
    openSettingsSection: (sectionId: string) => {
      setNavOpen(false);
      setSettingsSection(sectionId);
      setSettingsOpen(true);
    },
    closeSettings: () => {
      setSettingsOpen(false);
      if (isSettingsRoute && isNativeApp) {
        window.location.assign(appUrl('__native/close'));
      }
    },
    themeMode,
    setThemeMode,
    effectiveTheme,
    defaultBackend,
    setDefaultBackend,
    autoCollapseCompletedTurns,
    setAutoCollapseCompletedTurns,
    phoneDock,
    setPhoneDock,
    phoneHand,
    setPhoneHand,
    showReasoningSummaries,
    setShowReasoningSummaries: (enabled: boolean) => {
      setShowReasoningSummaries(enabled);
      try { localStorage.setItem('remote-codex-show-reasoning-summaries', String(enabled)); } catch { /* Private browsing may disable storage. */ }
    },
  };

  return (
    <HostCapabilitiesProvider scope={relayDeviceIdFromPath(location.pathname) ?? 'local'}>
    <AppShellNavContext.Provider value={shellNavValue}>
      <ScopedTourProvider>
      <div
        className={`bg-[var(--app-bg)] text-[var(--app-fg)] ${
          isViewportLockedRoute || isSettingsRoute
            ? 'fixed inset-0 overflow-hidden overscroll-none'
            : 'min-h-screen'
        }`}
      >
        <main
          className={`mx-auto w-full ${
            isThreadWorkspaceRoute ? 'max-w-none' : 'max-w-[1600px]'
          } ${
            isThreadWorkspaceRoute
              ? 'absolute inset-0 pb-0'
              : isViewportLockedRoute || isSettingsRoute
              ? 'absolute inset-0 pb-0 sm:pb-4'
              : 'pb-4'
          } ${
            isSettingsRoute
              ? 'pt-0'
              : isThreadWorkspaceRoute
                ? 'pt-0'
                : isWorkspacesRoute
                  ? 'pt-[env(safe-area-inset-top)] sm:pt-4'
                  : 'pt-4'
          } ${
            isViewportLockedRoute || isSettingsRoute
              ? 'overflow-hidden overscroll-none px-0'
              : 'px-4 sm:px-6'
          }`}
        >
          <section
            className={`min-w-0 ${
              isViewportLockedRoute
                ? isThreadDetailRoute
                  ? 'h-full min-h-0 overflow-hidden overscroll-none'
                  : 'h-full overflow-hidden overscroll-none'
                : ''
            }`}
          >
            <HostScopeRouteGate><Outlet /></HostScopeRouteGate>
          </section>
        </main>
      </div>
      <UnscopedOnly><AppShellSettingsDialog /></UnscopedOnly>
      </ScopedTourProvider>
    </AppShellNavContext.Provider>
    </HostCapabilitiesProvider>
  );
}

function AuthGate({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  const [state, setState] = useState<
    | { status: 'checking' }
    | { status: 'authenticated' }
    | { status: 'loginRequired'; session: AuthSessionDto; error: string | null }
    | { status: 'unavailable'; error: string }
  >({ status: 'checking' });

  async function checkSession(cancelled?: () => boolean) {
    try {
      const session = await fetchAuthSession();
      if (cancelled?.()) {
        return;
      }
      setState(
        !session.authRequired || session.authenticated
          ? { status: 'authenticated' }
          : { status: 'loginRequired', session, error: null },
      );
    } catch (caught) {
      if (cancelled?.()) {
        return;
      }
      if (caught instanceof ApiError && caught.statusCode === 401) {
        setState({
          status: 'loginRequired',
          session: {
            authenticated: false,
            username: null,
            expiresAt: null,
            mode: 'server',
            authRequired: true,
          },
          error: null,
        });
        return;
      }
      setState({
        status: 'unavailable',
        error: caught instanceof Error ? caught.message : translate("workbench.unableToCheckSupervisorAccess"),
      });
    }
  }

  useEffect(() => {
    let cancelled = false;
    void checkSession(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleLogin(input: { username: string; password: string }) {
    await login(input);
    setState({ status: 'authenticated' });
  }

  function handleRetry() {
    setState({ status: 'checking' });
    void checkSession();
  }

  if (state.status === 'checking') {
    return (
      <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] px-4 text-sm text-[var(--theme-fg-muted)]">
        {translate("workbench.checkingSupervisorAccess")}</main>
    );
  }

  if (state.status === 'unavailable') {
    return (
      <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] px-4 text-[var(--app-fg)]">
        <section className="w-full max-w-md rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-5 shadow-[var(--theme-shadow)] sm:p-6">
          <p className="text-xs font-semibold uppercase tracking-[0.22em] text-[var(--theme-fg-muted)]">
            {translate("workbench.supervisorAccess")}</p>
          <h1 className="mt-2 text-2xl font-semibold tracking-normal text-[var(--theme-fg)]">
            {translate("workbench.unableToReachSupervisor")}</h1>
          <p className="mt-3 text-sm leading-6 text-[var(--theme-fg-muted)]">
            {state.error}
          </p>
          <button
            className="mt-5 h-11 rounded-lg bg-[var(--theme-accent-solid)] px-4 text-sm font-semibold text-[var(--theme-accent-solid-fg)] transition hover:bg-[var(--theme-accent-solid-hover)] focus:outline-none focus:ring-2 focus:ring-[var(--theme-accent-border)]"
            onClick={handleRetry}
            type="button"
          >
            {translate("workbench.retry")}</button>
        </section>
      </main>
    );
  }

  if (state.status === 'loginRequired') {
    return (
      <>
        {state.error && (
          <div className="fixed left-1/2 top-4 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 rounded-lg border border-[var(--status-warning-border)] bg-[var(--status-warning-bg)] px-3 py-2 text-sm text-[var(--status-warning-fg)]">
            {state.error}
          </div>
        )}
        <LoginPage onLogin={handleLogin} />
      </>
    );
  }

  return children;
}

function RelayGate({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  const location = useLocation();
  const [state, setState] = useState<
    | { status: 'checking' }
    | { status: 'authenticated'; session: RelaySessionDto }
    | { status: 'loginRequired' }
  >({ status: 'checking' });

  useEffect(() => {
    let cancelled = false;
    fetchRelaySession()
      .then((session) => {
        if (cancelled) {
          return;
        }
        setState(
          session.authenticated && session.user?.role !== 'admin'
            ? { status: 'authenticated', session }
            : { status: 'loginRequired' },
        );
      })
      .catch(() => {
        if (!cancelled) {
          setState({ status: 'loginRequired' });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (state.status === 'checking') {
    return (
      <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] px-4 text-sm text-[var(--theme-fg-muted)]">
        {translate("workbench.checkingRelayAccess")}</main>
    );
  }

  if (state.status === 'loginRequired') {
    const returnTo = `${location.pathname}${location.search}${location.hash}`;
    return (
      <Navigate
        to={`/relay-portal?returnTo=${encodeURIComponent(returnTo)}`}
        replace
      />
    );
  }

  return <AccountSessionGate>{children}</AccountSessionGate>;
}

function SupervisorAccessGate({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  return relayModeActive() ? <RelayGate>{children}</RelayGate> : <AuthGate>{children}</AuthGate>;
}

function RootRoute() {
  const { locale: i18nLocale } = useI18n();
  return relayModeActive() ? <RelayHomePage /> : <Navigate to="/workspaces" replace />;
}

function SupervisorRoutes({
  themeMode,
  setThemeMode,
  effectiveTheme,
}: {
  themeMode: ThemeMode;
  setThemeMode: (mode: ThemeMode) => void;
  effectiveTheme: 'light' | 'dark';
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <Routes>
      <Route
        element={
          <AppShell
            themeMode={themeMode}
            setThemeMode={setThemeMode}
            effectiveTheme={effectiveTheme}
          />
        }
      >
        <Route path="/" element={<RootRoute />} />
        <Route path="/workspaces" element={<WorkspacesPage />} />
        <Route path="/workspaces/new" element={<WorkspaceNewPage />} />
        <Route path="/relay-account" element={<RelayAccountPage />} />
        <Route path="/relay-devices" element={<RelayDevicesPage />} />
        <Route path="/relay-settings" element={<RelaySettingsPage />} />
        <Route path="/threads" element={<ThreadsPage />} />
        <Route path="/threads/import" element={<ThreadImportPage />} />
        <Route path="/threads/new" element={<ThreadNewPage />} />
        <Route path="/threads/:id" element={<ThreadDetailPage />} />
        <Route path="/devices/:relayDeviceId/workspaces" element={<WorkspacesPage />} />
        <Route path="/devices/:relayDeviceId/workspaces/new" element={<WorkspaceNewPage />} />
        <Route path="/devices/:relayDeviceId/threads" element={<ThreadsPage />} />
        <Route path="/devices/:relayDeviceId/threads/import" element={<ThreadImportPage />} />
        <Route path="/devices/:relayDeviceId/threads/new" element={<ThreadNewPage />} />
        <Route path="/devices/:relayDeviceId/threads/:id" element={<ThreadDetailPage />} />
        <Route
          path="*"
          element={
            <Navigate
              to={relayModeActive() ? '/relay-devices' : '/workspaces'}
              replace
            />
          }
        />
      </Route>
    </Routes>
  );
}

export function App() {
  const { locale: i18nLocale } = useI18n();
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => readInitialThemeMode());
  const [systemTheme, setSystemTheme] = useState<'light' | 'dark'>(() =>
    systemThemePreference(),
  );
  const themeRegistry = useThemeRegistry();
  const appliedTheme = themeRegistry.applied;
  // A single-mode pack fixes the color mode while it is applied; the user's
  // setting stays stored and returns with a pack that has both modes.
  const effectiveTheme = effectiveColorMode(themeMode === 'system' ? systemTheme : themeMode, themeRegistry);
  // The mode the shells show (`data-theme-mode` selects their palettes too).
  const shownThemeMode: ThemeMode = themeRegistry.fixedMode ?? themeMode;

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
      return;
    }

    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    const update = () => {
      setSystemTheme(mediaQuery.matches ? 'dark' : 'light');
    };

    update();
    if (typeof mediaQuery.addEventListener === 'function') {
      mediaQuery.addEventListener('change', update);
    } else {
      mediaQuery.addListener(update);
    }
    return () => {
      if (typeof mediaQuery.removeEventListener === 'function') {
        mediaQuery.removeEventListener('change', update);
      } else {
        mediaQuery.removeListener(update);
      }
    };
  }, []);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }

    window.localStorage.setItem(THEME_STORAGE_KEY, themeMode);
  }, [themeMode]);

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.dataset.themeMode = shownThemeMode;
    root.dataset.themeEffective = effectiveTheme;
    root.style.colorScheme = effectiveTheme;
    document
      .querySelector<HTMLMetaElement>('meta[name="theme-color"]')
      ?.setAttribute('content', themeChromeColor(effectiveTheme));
  }, [effectiveTheme, shownThemeMode, appliedTheme]);

  return (
    <div className="theme-shell theme-scrollbar">
      <BrowserRouter basename={webBase}>
        <WorkspaceDocumentNavigationGuard>
        <DocumentTitleUpdater />
        <AccountUnlockPrompt />
        <DeviceEnrollmentMonitor />
        <RoutePluginProvider>
          <Routes>
            <Route path="/shared" element={<SharedInvitePage />} />
            <Route path="/devices/:deviceId/ports/:mappingId" element={<RelayGate><PortPreviewPage /></RelayGate>} />
            <Route path="/s/:id" element={<PublicThreadPage />} />
            <Route path="/relay-guide" element={<RelayGuidePage />} />
            <Route path="/help" element={<HelpPage />} />
            <Route path="/help/:topic" element={<HelpPage />} />
            <Route path="/relay-portal" element={<RelayPortalPage />} />
            <Route path="/relay-admin" element={<RelayAdminPage />} />
            <Route
              path="/*"
              element={
                <SupervisorAccessGate>
                  <SupervisorRoutes
                    themeMode={shownThemeMode}
                    setThemeMode={setThemeMode}
                    effectiveTheme={effectiveTheme}
                  />
                </SupervisorAccessGate>
              }
            />
          </Routes>
        </RoutePluginProvider>
        </WorkspaceDocumentNavigationGuard>
      </BrowserRouter>
    </div>
  );
}
