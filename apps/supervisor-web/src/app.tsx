import { installNotificationRouteResponder } from './lib/relayPush';
import { PublicThreadPage } from './pages/PublicThreadPage';
import {
  BrowserRouter,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
} from 'react-router-dom';
import { lazy, Suspense, useEffect, useMemo, useState, type ReactNode } from 'react';
import { PluginProvider } from '@remote-codex/thread-ui';

import type {
  AgentBackendIdDto,
  AuthSessionDto,
  RelaySessionDto,
} from '../../../packages/shared/src/index';
import {
  defaultAgentBackendId,
  normalizeAgentBackendId,
} from '@remote-codex/shared';
import {
  AppShellNavContext,
  type ThemeMode,
} from './components/AppShellNavContext';
import {
  AppShellSettingsDialog,
} from './components/AppShellNavigation';
import { LoginPage } from './pages/LoginPage';
import { RelayAccountPage } from './pages/RelayAccountPage';
import { RelayAdminPage } from './pages/RelayAdminPage';
import { RelayDevicesPage } from './pages/RelayDevicesPage';
import { RelayGuidePage } from './pages/RelayGuidePage';
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
  HOSTED_VM_WAKE_EVENT,
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

const DesktopEntry = lazy(() => import('./desktop/DesktopEntry'));

const THEME_STORAGE_KEY = 'remote-codex-theme-mode';
const BACKEND_STORAGE_KEY = 'remote-codex-default-backend';
const AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY =
  'remote-codex-auto-collapse-completed-turns';

function RoutePluginProvider({ children }: { children: ReactNode }) {
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

  return window.localStorage.getItem(AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY) !== 'false';
}

function formatDocumentTitle(pageTitle?: string | null) {
  const trimmed = pageTitle?.trim();
  return trimmed || 'Remote Codex';
}

function routeDocumentTitle(pathname: string) {
  if (pathname === '/' || pathname === '/relay-portal') {
    return 'Relay Portal';
  }
  if (pathname === '/relay-guide') {
    return 'Relay Setup';
  }
  if (pathname === '/relay-admin') {
    return 'Relay Admin';
  }
  if (pathname === '/relay-account') {
    return 'Account';
  }
  if (pathname === '/relay-devices') {
    return 'Devices and Shared Sessions';
  }
  if (pathname === '/relay-settings') {
    return 'Settings';
  }
  if (
    pathname === '/workspaces' ||
    /^\/devices\/[^/]+\/workspaces$/.test(pathname)
  ) {
    return 'Workspaces';
  }
  if (
    pathname === '/workspaces/new' ||
    /^\/devices\/[^/]+\/workspaces\/new$/.test(pathname)
  ) {
    return 'New Workspace';
  }
  if (
    pathname === '/threads' ||
    /^\/devices\/[^/]+\/threads$/.test(pathname)
  ) {
    return 'Threads';
  }
  if (
    pathname === '/threads/import' ||
    /^\/devices\/[^/]+\/threads\/import$/.test(pathname)
  ) {
    return 'Import Thread';
  }
  if (
    pathname === '/threads/new' ||
    /^\/devices\/[^/]+\/threads\/new$/.test(pathname)
  ) {
    return 'New Thread';
  }
  if (
    /^\/threads\/[^/]+$/.test(pathname) ||
    /^\/devices\/[^/]+\/threads\/[^/]+$/.test(pathname)
  ) {
    return 'Thread';
  }

  return null;
}

function DocumentTitleUpdater() {
  useEffect(installNotificationRouteResponder, []);
  const location = useLocation();

  useEffect(() => {
    document.title = formatDocumentTitle(routeDocumentTitle(location.pathname));
  }, [location.pathname]);

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

function AppShell({
  themeMode,
  setThemeMode,
  effectiveTheme,
}: {
  themeMode: ThemeMode;
  setThemeMode: (mode: ThemeMode) => void;
  effectiveTheme: 'light' | 'dark';
}) {
  const [navOpen, setNavOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [showReasoningSummaries, setShowReasoningSummaries] = useState(() => {
    try { return localStorage.getItem('remote-codex-show-reasoning-summaries') === 'true'; } catch { return false; }
  });
  const [defaultBackend, setDefaultBackendState] = useState<AgentBackendIdDto>(readInitialBackend);
  const [autoCollapseCompletedTurns, setAutoCollapseCompletedTurnsState] = useState(
    readInitialAutoCollapseCompletedTurns,
  );
  const [hostedVmWake, setHostedVmWake] = useState<{
    attempt: number;
    startedAt: number;
  } | null>(null);
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

  useEffect(() => {
    const onHostedVmWake = (event: Event) => {
      const detail = (event as CustomEvent<{
        state: 'starting' | 'connected';
        attempt: number;
      }>).detail;
      if (detail.state === 'connected') {
        setHostedVmWake(null);
        return;
      }
      setHostedVmWake((current) => ({
        attempt: detail.attempt,
        startedAt: current?.startedAt ?? Date.now(),
      }));
    };
    window.addEventListener(HOSTED_VM_WAKE_EVENT, onHostedVmWake);
    return () =>
      window.removeEventListener(HOSTED_VM_WAKE_EVENT, onHostedVmWake);
  }, []);

  function setDefaultBackend(backend: AgentBackendIdDto) {
    setDefaultBackendState(backend);
    window.localStorage.setItem(BACKEND_STORAGE_KEY, backend);
  }

  function setAutoCollapseCompletedTurns(enabled: boolean) {
    setAutoCollapseCompletedTurnsState(enabled);
    window.localStorage.setItem(
      AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY,
      enabled ? 'true' : 'false',
    );
  }

  const shellNavValue = {
    navOpen,
    openNav: () => setNavOpen(true),
    toggleNav: () => setNavOpen((current) => !current),
    closeNav: () => setNavOpen(false),
    settingsOpen,
    openSettings: () => {
      setNavOpen(false);
      setSettingsOpen(true);
    },
    closeSettings: () => {
      setSettingsOpen(false);
      if (isSettingsRoute && isNativeApp) {
        window.location.assign('/__native/close');
      }
    },
    themeMode,
    setThemeMode,
    effectiveTheme,
    defaultBackend,
    setDefaultBackend,
    autoCollapseCompletedTurns,
    setAutoCollapseCompletedTurns,
    showReasoningSummaries,
    setShowReasoningSummaries: (enabled: boolean) => {
      setShowReasoningSummaries(enabled);
      try { localStorage.setItem('remote-codex-show-reasoning-summaries', String(enabled)); } catch { /* Private browsing may disable storage. */ }
    },
  };

  return (
    <AppShellNavContext.Provider value={shellNavValue}>
      {window.remoteCodexHost ? (
        <Suspense fallback={null}><DesktopEntry><section className="h-full min-h-0"><Outlet /></section></DesktopEntry></Suspense>
      ) : <div
        className={`bg-[var(--app-bg)] text-[var(--app-fg)] ${
          isViewportLockedRoute || isSettingsRoute
            ? 'fixed inset-0 overflow-hidden overscroll-none'
            : 'min-h-screen'
        }`}
      >
        {hostedVmWake ? (
          <div
            aria-live="polite"
            className="fixed inset-x-4 top-4 z-[80] mx-auto max-w-2xl overflow-hidden rounded-lg border border-[var(--status-warning-border)] bg-[var(--theme-panel)] shadow-[var(--theme-shadow)]"
            role="status"
          >
            <div className="flex items-center justify-between gap-4 px-4 py-3">
              <div className="min-w-0">
                <p className="text-sm font-semibold text-[var(--theme-fg)]">
                  Starting hosted VM
                </p>
                <p className="truncate text-xs text-[var(--theme-fg-muted)]">
                  Waiting for the supervisor. Pages will resume automatically.
                </p>
              </div>
              <span className="shrink-0 text-xs tabular-nums text-[var(--theme-fg-muted)]">
                Check {hostedVmWake.attempt}
              </span>
            </div>
            <div className="h-1 overflow-hidden bg-[var(--theme-muted)]">
              <div className="h-full w-1/3 animate-pulse bg-[var(--theme-accent-solid)]" />
            </div>
          </div>
        ) : null}
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
            <Outlet />
          </section>
        </main>
      </div>}
      <AppShellSettingsDialog />
    </AppShellNavContext.Provider>
  );
}

function AuthGate({ children }: { children: React.ReactNode }) {
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
        error: caught instanceof Error ? caught.message : 'Unable to check supervisor access.',
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
        Checking supervisor access...
      </main>
    );
  }

  if (state.status === 'unavailable') {
    return (
      <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] px-4 text-[var(--app-fg)]">
        <section className="w-full max-w-md rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-5 shadow-[var(--theme-shadow)] sm:p-6">
          <p className="text-xs font-semibold uppercase tracking-[0.22em] text-[var(--theme-fg-muted)]">
            Supervisor Access
          </p>
          <h1 className="mt-2 text-2xl font-semibold tracking-normal text-[var(--theme-fg)]">
            Unable to reach supervisor
          </h1>
          <p className="mt-3 text-sm leading-6 text-[var(--theme-fg-muted)]">
            {state.error}
          </p>
          <button
            className="mt-5 h-11 rounded-lg bg-[var(--theme-accent-solid)] px-4 text-sm font-semibold text-[var(--theme-accent-solid-fg)] transition hover:bg-[var(--theme-accent-solid-hover)] focus:outline-none focus:ring-2 focus:ring-[var(--theme-accent-border)]"
            onClick={handleRetry}
            type="button"
          >
            Retry
          </button>
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
        Checking relay access...
      </main>
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

  return children;
}

function SupervisorAccessGate({ children }: { children: React.ReactNode }) {
  return relayModeActive() ? <RelayGate>{children}</RelayGate> : <AuthGate>{children}</AuthGate>;
}

function RootRoute() {
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
        <Route path="/desktop" element={window.remoteCodexHost ? <></> : <Navigate to="/" replace />} />
        <Route path="/desktop/devices" element={window.remoteCodexHost ? <></> : <Navigate to="/relay-devices" replace />} />
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
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => readInitialThemeMode());
  const [systemTheme, setSystemTheme] = useState<'light' | 'dark'>(() =>
    systemThemePreference(),
  );
  const effectiveTheme = themeMode === 'system' ? systemTheme : themeMode;

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

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.themeMode = themeMode;
    root.dataset.themeEffective = effectiveTheme;
    root.style.colorScheme = effectiveTheme;
    document
      .querySelector<HTMLMetaElement>('meta[name="theme-color"]')
      ?.setAttribute('content', effectiveTheme === 'dark' ? '#171713' : '#f3f6f7');
  }, [effectiveTheme, themeMode]);

  return (
    <div className="theme-shell theme-scrollbar">
      <BrowserRouter>
        <DocumentTitleUpdater />
        <RoutePluginProvider>
          <Routes>
            <Route path="/s/:id" element={<PublicThreadPage />} />
            <Route path="/relay-guide" element={<RelayGuidePage />} />
            <Route path="/relay-portal" element={<RelayPortalPage />} />
            <Route path="/relay-admin" element={<RelayAdminPage />} />
            <Route
              path="/*"
              element={
                <SupervisorAccessGate>
                  <SupervisorRoutes
                    themeMode={themeMode}
                    setThemeMode={setThemeMode}
                    effectiveTheme={effectiveTheme}
                  />
                </SupervisorAccessGate>
              }
            />
          </Routes>
        </RoutePluginProvider>
      </BrowserRouter>
    </div>
  );
}
