import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { LoginVerification, RelaySecurityPanel, usePendingLogin } from '../components/RelaySecurity';
import {
  FormEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Check,
  KeyRound,
  LogOut,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  X,
} from 'lucide-react';

import type {
  RelayAccessGrantDto,
  RelayAdminDeviceDto,
  RelayAdminSummaryDto,
  RelayRegistrationSettingsDto,
  RelaySessionDto,
  RelaySessionShareDto,
  RelayUserDto,
} from '@pockymoe/shared';
import { LoginPage } from './LoginPage';
import { useDialogLifecycle } from '../components/useDialogLifecycle';
import {
  ApiError,
  approveRelayRegistration,
  deleteRelayAdminUser,
  enableRelayMode,
  fetchRelayAdmin,
  fetchRelayAdminSession,
  rejectRelayRegistration,
  relayAdminLogout,
  relayAdminLogin,
  resetRelayAdminUserPassword,
  setRelayUserEnabled,
  updateRelayRegistrationSettings,
} from '../lib/api';

type AdminTab =
  | 'overview'
  | 'users'
  | 'devices'
  | 'shares'
  | 'settings'
  | 'security';
const adminTabs: AdminTab[] = [
  'overview',
  'users',
  'devices',
  'shares',
  'settings',
  'security',
];
type SortDirection = 'asc' | 'desc';
type UserSortKey =
  | 'username'
  | 'enabled'
  | 'lastSeenAt'
  | 'deviceCount'
  | 'createdAt';
type DeviceSortKey =
  | 'name'
  | 'ownerUsername'
  | 'connected'
  | 'lastActivity'
  | 'createdAt'
  | 'workspaceCount'
  | 'threadCount';

function errorMessage(caught: unknown) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : translate("devices.unableToUpdateRelayAdminState");
}

function adminTabFromSearch(search: string): AdminTab {
  const requested = new URLSearchParams(search).get('tab');
  return adminTabs.includes(requested as AdminTab)
    ? (requested as AdminTab)
    : 'overview';
}

function unsupportedAdminStatus(caught: unknown) {
  if (
    caught instanceof ApiError &&
    (caught.statusCode === 404 || caught.statusCode === 501)
  ) {
    return caught.statusCode;
  }
  return null;
}

export function RelayAdminPage() {
  const { locale: i18nLocale } = useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const [summary, setSummary] = useState<RelayAdminSummaryDto | null>(null);
  const [adminSession, setAdminSession] = useState<RelaySessionDto | null>(null);
  const [loading, setLoading] = useState(true);
  // Why the last load or action failed, shown as a mark after its control
  // (`at`): 'load', 'settings', `user:<id>`, `approve:<id>`, `reject:<id>`.
  const [error, setError] = useState<AdminFailure>(null);
  const [unsupportedStatus, setUnsupportedStatus] = useState<number | null>(null);
  const [challenge,setChallenge]=usePendingLogin();
  const [loginRequired, setLoginRequired] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [tab, setTab] = useState<AdminTab>(() =>
    adminTabFromSearch(location.search),
  );
  const [settingsDraft, setSettingsDraft] =
    useState<RelayRegistrationSettingsDto | null>(null);

  async function load(options: { showLoading?: boolean } = {}) {
    if (options.showLoading !== false) {
      setLoading(true);
    }
    setError(null);
    setUnsupportedStatus(null);
    try {
      enableRelayMode();
      const result = await fetchRelayAdmin();
      setSummary(result);
      setSettingsDraft(result.settings);
      setLoginRequired(false);
    } catch (caught) {
      if (
        caught instanceof ApiError &&
        (caught.statusCode === 401 || caught.statusCode === 403)
      ) {
        setSummary(null);
        setAdminSession(null);
        setLoginRequired(true);
        setError(null);
      } else {
        const status = unsupportedAdminStatus(caught);
        if (status) {
          setSummary(null);
          setUnsupportedStatus(status);
          setError(null);
        } else {
          setError({ at: 'load', message: errorMessage(caught) });
        }
      }
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    let cancelled = false;

    async function initialize() {
      setLoading(true);
      setError(null);
      setUnsupportedStatus(null);
      try {
        enableRelayMode();
        const session = await fetchRelayAdminSession();
        if (cancelled) {
          return;
        }
        if (!session.authenticated || session.user?.role !== 'admin') {
          setAdminSession(null);
          setSummary(null);
          setLoginRequired(true);
          setLoading(false);
          return;
        }
        setAdminSession(session);
        setLoginRequired(false);
        await load({ showLoading: false });
      } catch (caught) {
        if (cancelled) {
          return;
        }
        if (
          caught instanceof ApiError &&
          (caught.statusCode === 401 || caught.statusCode === 403)
        ) {
          setAdminSession(null);
          setSummary(null);
          setLoginRequired(true);
          setError(null);
        } else {
          setError({
            at: 'load',
            message: caught instanceof Error
              ? translate("devices.unableToVerifyTheRelayAdminSession", { value1: caught.message })
              : translate("devices.unableToVerifyTheRelayAdminSession_f228be"),
          });
        }
        setLoading(false);
      }
    }

    void initialize();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const nextTab = adminTabFromSearch(location.search);
    setTab(nextTab);

    const searchParams = new URLSearchParams(location.search);
    if (searchParams.get('tab') !== nextTab) {
      searchParams.set('tab', nextTab);
      navigate(
        `${location.pathname}?${searchParams.toString()}`,
        { replace: true },
      );
    }
  }, [location.pathname, location.search, navigate]);

  const totals = useMemo(() => {
    const users = summary?.users ?? [];
    const devices = summary?.devices ?? [];
    return {
      users: users.length,
      enabledUsers: users.filter((user) => user.enabled).length,
      devices: devices.length,
      onlineDevices: devices.filter((device) => device.connected).length,
      // Thread shares and grants (thread, workspace, whole device) are both shares.
      shares: (summary?.shares.length ?? 0) + (summary?.grants?.length ?? 0),
    };
  }, [summary]);

  async function updateUser(userId: string, enabled: boolean) {
    setBusyKey(userId);
    setError(null);
    try {
      const updated = await setRelayUserEnabled(userId, enabled);
      setSummary((current) => replaceAdminUser(current, updated));
    } catch (caught) {
      setError({ at: `user:${userId}`, message: errorMessage(caught) });
    } finally {
      setBusyKey(null);
    }
  }

  async function deleteUser(userId: string) {
    setBusyKey(`delete:${userId}`);
    setError(null);
    try {
      await deleteRelayAdminUser(userId);
      await load({ showLoading: false });
    } finally {
      setBusyKey(null);
    }
  }

  async function resetUserPassword(userId: string, password: string) {
    setBusyKey(`reset:${userId}`);
    setError(null);
    try {
      const updated = await resetRelayAdminUserPassword(userId, password);
      setSummary((current) => replaceAdminUser(current, updated));
    } finally {
      setBusyKey(null);
    }
  }

  async function saveSettings(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!settingsDraft) {
      return;
    }
    setBusyKey('settings');
    setError(null);
    try {
      const result = await updateRelayRegistrationSettings(settingsDraft);
      setSummary((current) =>
        current
          ? {
              ...current,
              registrationEnabled: result.registrationEnabled,
              settings: result.settings,
            }
          : current,
      );
      setSettingsDraft(result.settings);
    } catch (caught) {
      setError({ at: 'settings', message: errorMessage(caught) });
    } finally {
      setBusyKey(null);
    }
  }

  async function reviewRegistration(
    requestId: string,
    action: 'approve' | 'reject',
  ) {
    const request = summary?.pendingRegistrations.find((item) => item.id === requestId);
    const values = { name: request?.username ?? requestId, email: request?.email ?? '' };
    const confirmed = await confirmAsync(action === 'approve'
      ? {
          title: translate('devices.approveRegistrationTitle'),
          description: translate('devices.approveRegistrationBody', values),
          confirmLabel: translate('devices.approve'),
          tone: 'primary',
        }
      : {
          title: translate('devices.rejectRegistrationTitle'),
          description: translate('devices.rejectRegistrationBody', values),
          confirmLabel: translate('devices.reject'),
        });
    if (!confirmed) return;
    setBusyKey(`${action}:${requestId}`);
    setError(null);
    try {
      if (action === 'approve') {
        await approveRelayRegistration(requestId);
      } else {
        await rejectRelayRegistration(requestId);
      }
      await load({ showLoading: false });
    } catch (caught) {
      setError({ at: `${action}:${requestId}`, message: errorMessage(caught) });
    } finally {
      setBusyKey(null);
    }
  }

  async function handleAdminLogin(input: {
    username: string;
    password: string;
  }) {
    const result = await relayAdminLogin(input);
    if(result.challengeRequired){setChallenge({challengeRequired:true,authenticator:Boolean(result.authenticator),passkey:Boolean(result.passkey)});return;}
    if (!result.session.authenticated || result.session.user?.role !== 'admin') {
      throw new Error(translate("devices.thisAccountDoesNotHaveRelayAdmin"));
    }
    setAdminSession(result.session);
    setLoginRequired(false);
    await load();
  }

  function selectTab(nextTab: AdminTab) {
    setTab(nextTab);
    const searchParams = new URLSearchParams(location.search);
    searchParams.set('tab', nextTab);
    navigate(`${location.pathname}?${searchParams.toString()}`, { replace: true });
  }

  function handleTabKeyDown(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    currentTab: AdminTab,
  ) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
      return;
    }

    event.preventDefault();
    const currentIndex = adminTabs.indexOf(currentTab);
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? adminTabs.length - 1
          : event.key === 'ArrowRight'
            ? (currentIndex + 1) % adminTabs.length
            : (currentIndex - 1 + adminTabs.length) % adminTabs.length;
    const nextTab = adminTabs[nextIndex];
    if (!nextTab) {
      return;
    }
    selectTab(nextTab);
    window.requestAnimationFrame(() => {
      document.getElementById(`relay-admin-tab-${nextTab}`)?.focus();
    });
  }

  if (challenge) return <main className="flex min-h-screen items-center justify-center p-4"><LoginVerification challenge={challenge} onBack={()=>setChallenge(null)} onSuccess={async()=>{setChallenge(null);setAdminSession(await fetchRelayAdminSession());await load();}}/></main>;

  if (loginRequired) {
    return (
      <LoginPage
        description={translate("devices.useTheRelayAdminCredentialsForThis")}
        eyebrow={translate("devices.relayAdmin")}
        onLogin={handleAdminLogin}
      />
    );
  }

  return (
    <main className="min-h-screen bg-[var(--app-bg)] px-4 pt-[env(safe-area-inset-top)] text-[var(--app-fg)] sm:px-6" data-page="admin">
      <div className="product-page product-page-wide flex flex-col gap-5">
        <header className="product-page-header !items-start max-lg:flex-col">
          <div>
            <p className="product-eyebrow">
              {translate("devices.relayAdmin")}</p>
            <h1 className="product-title mt-1">
              {translate("devices.administration")}</h1>
            <p className="product-description mt-1">
              {translate("devices.manageRelayAccessConnectedDevicesAndRegistration")}</p>
          </div>
          <div className="flex w-full flex-wrap items-end gap-2 lg:w-auto lg:justify-end">
            <Link className="relay-button-secondary" to="/">
              {translate("devices.relayHome")}</Link>
            <span className="flex items-center gap-1">
              <button
                aria-label={translate("devices.refreshAdminData")}
                className="product-icon-button"
                onClick={() => void load()}
                title={translate("devices.refreshAdminData")}
                type="button"
              >
                <RefreshCw className="h-4 w-4" />
              </button>
              {error?.at === 'load' && summary ? <ErrorMark message={error.message} onRetry={() => void load()} /> : null}
            </span>
            <RelayAdminUserMenu
              session={adminSession}
              onLogout={() => {
                setAdminSession(null);
                setSummary(null);
                setLoginRequired(true);
              }}
            />
          </div>
        </header>

        {loading ? (
          <section className="product-panel" aria-busy="true" aria-label={translate("devices.loadingRelayAdministration")}>
            <div className="product-row min-h-24">
              <div className="min-w-0 flex-1 space-y-3">
                <div className="product-skeleton h-4 w-40" />
                <div className="product-skeleton h-3 w-72 max-w-full" />
              </div>
            </div>
          </section>
        ) : unsupportedStatus ? (
          <AdminCompatibilityState
            statusCode={unsupportedStatus}
            onRetry={() => void load()}
          />
        ) : error && !summary ? (
          <section className="product-panel">
            <div className="p-5 sm:p-6">
              <p className="flex items-center gap-1.5 text-base font-semibold text-[var(--theme-fg)]">
                {translate("devices.adminDataIsUnavailable")}
                <ErrorMark message={error.message} />
              </p>
              <div className="mt-4 flex flex-wrap gap-2">
                <button
                  className="relay-button-primary"
                  onClick={() => void load()}
                  type="button"
                >
                  {translate("devices.retry")}</button>
                <Link className="relay-button-secondary" to="/">
                  {translate("devices.relayHome")}</Link>
              </div>
            </div>
          </section>
        ) : summary ? (
          <>
            <section className="product-panel" aria-label={translate("devices.relaySummary")}>
              <dl className="grid grid-cols-2 sm:grid-cols-4">
                <MetricStat
                  label={translate("devices.users")}
                  value={totals.users}
                  detail={translate("devices.enabled_5cd672", { value1: totals.enabledUsers })}
                />
                <MetricStat
                  label={translate("devices.devices")}
                  value={totals.devices}
                  detail={translate("devices.online", { value1: totals.onlineDevices })}
                />
                <MetricStat
                  label={translate("devices.activeShares")}
                  value={totals.shares}
                  detail=""
                />
                <MetricStat
                  label={translate("devices.pendingRegistrations")}
                  value={summary.pendingRegistrations.length}
                  detail=""
                />
              </dl>
            </section>

            <div
              aria-label={translate("devices.relayAdministrationSections")}
              aria-orientation="horizontal"
              className="product-segmented w-full"
              role="tablist"
            >
              {adminTabs.map((item) => (
                <button
                  aria-controls={`relay-admin-panel-${item}`}
                  aria-selected={tab === item}
                  className="product-segment min-h-11 shrink-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
                  id={`relay-admin-tab-${item}`}
                  key={item}
                  onClick={() => selectTab(item)}
                  onKeyDown={(event) => handleTabKeyDown(event, item)}
                  role="tab"
                  tabIndex={tab === item ? 0 : -1}
                  type="button"
                >
                  {tabLabel(item)}
                </button>
              ))}
            </div>

            <div
              aria-labelledby={`relay-admin-tab-${tab}`}
              className="min-w-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
              id={`relay-admin-panel-${tab}`}
              role="tabpanel"
              tabIndex={0}
            >
              {tab === 'overview' ? <Overview summary={summary} /> : null}
              {tab === 'users' ? (
                <UsersTable
                  busyKey={busyKey}
                  failure={error}
                  onDeleteUser={deleteUser}
                  onResetPassword={resetUserPassword}
                  onUpdateUser={updateUser}
                  users={summary.users}
                />
              ) : null}
              {tab === 'devices' ? (
                <DevicesPanel devices={summary.devices} users={summary.users} />
              ) : null}
              {tab === 'security' ? <RelaySecurityPanel realm="relay-admin" /> : null}
              {tab === 'shares' ? <SharesTable grants={summary.grants ?? []} shares={summary.shares} /> : null}
              {tab === 'settings' && settingsDraft ? (
                <SettingsPanel
                  busy={busyKey === 'settings'}
                  draft={settingsDraft}
                  failure={error}
                  onChange={setSettingsDraft}
                  onReviewRegistration={reviewRegistration}
                  onSave={saveSettings}
                  pending={summary.pendingRegistrations}
                  reviewBusyKey={busyKey}
                />
              ) : null}
            </div>
          </>
        ) : null}
      </div>
    </main>
  );
}

function AdminCompatibilityState({
  onRetry,
  statusCode,
}: {
  onRetry: () => void;
  statusCode: number;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <section className="product-panel" role="status">
      <div className="p-5 sm:p-6">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">
            {translate("devices.adminAPIIsNotAvailable")}</h2>
          <span className="rounded-full border border-[var(--status-warning-border)] bg-[var(--status-warning-bg)] px-2 py-0.5 text-xs font-medium text-[var(--status-warning-fg)]">
            HTTP {statusCode}
          </span>
          {/* Why, and what to do, open from the mark. */}
          <ErrorMark
            kind="unavailable"
            message={`${translate("devices.thisRelayServerDoesNotExposeThe")}\n${translate("devices.updateAndRestartTheRelayServerWith")}`}
          />
        </div>
        <div className="mt-5 flex flex-wrap gap-2">
          <button className="relay-button-primary" onClick={onRetry} type="button">
            {translate("devices.retry")}</button>
          <Link className="relay-button-secondary" to="/">
            {translate("devices.relayHome")}</Link>
        </div>
      </div>
    </section>
  );
}

function MetricStat({
  label,
  value,
  detail,
}: {
  label: string;
  value: number;
  detail: string;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div className="min-w-0 px-4 py-3.5 sm:border-l sm:border-[var(--theme-border)] sm:first:border-l-0">
      <dt className="truncate text-xs font-medium text-[var(--theme-fg-muted)]">
        {label}
      </dt>
      <dd className="mt-1 flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="text-xl font-semibold tabular-nums text-[var(--theme-fg)]">
          {value.toLocaleString(getLocale())}
        </span>
        <span className="text-xs text-[var(--theme-fg-muted)]">{detail}</span>
      </dd>
    </div>
  );
}

function RelayAdminUserMenu({
  onLogout,
  session,
}: {
  onLogout: () => void;
  session: RelaySessionDto | null;
}) {
  const { locale: i18nLocale } = useI18n();
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const wrapperRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    const focusTimer = window.setTimeout(() => {
      menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    }, 0);

    function handlePointerDown(event: PointerEvent) {
      if (event.target instanceof Node && !wrapperRef.current?.contains(event.target)) {
        setOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        setOpen(false);
        triggerRef.current?.focus();
      }
    }

    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [open]);

  const user = session?.user ?? null;
  if (!user) {
    return null;
  }

  async function logout() {
    try {
      await relayAdminLogout();
    } catch {
      // The local admin token is cleared before the session refresh request.
    } finally {
      setOpen(false);
      onLogout();
    }
  }

  function handleMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }

  return (
    <div
      className="relative z-40 shrink-0"
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
        aria-label={translate("devices.relayAdminMenuFor", { value1: user.username })}
        className="inline-flex h-11 w-11 items-center justify-center rounded-full border border-[var(--theme-border)] bg-[var(--theme-panel)] text-sm font-semibold text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setOpen(true);
          }
        }}
        ref={triggerRef}
        type="button"
      >
        {initials(user.username)}
      </button>
      {open ? (
        <div
          className="absolute right-0 mt-2 w-64 overflow-hidden rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-[var(--theme-shadow)]"
          id={menuId}
          onKeyDown={handleMenuKeyDown}
          ref={menuRef}
          role="menu"
        >
          <div className="border-b border-[var(--theme-border)] px-3 py-2">
            <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
              {user.username}
            </p>
            <p className="truncate text-xs text-[var(--theme-fg-muted)]">
              {user.email}
            </p>
            <p className="mt-1 text-[11px] uppercase tracking-[0.14em] text-[var(--theme-fg-muted)]">
              {translate("devices.adminSession")}</p>
          </div>
          <button
            className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 text-left text-sm text-[var(--status-danger-fg)] transition hover:bg-[var(--status-danger-bg)] focus-visible:bg-[var(--status-danger-bg)]"
            onClick={() => void logout()}
            role="menuitem"
            type="button"
          >
            <LogOut className="h-4 w-4" />
            {translate("devices.logoutAdmin")}</button>
        </div>
      ) : null}
    </div>
  );
}

function Overview({ summary }: { summary: RelayAdminSummaryDto }) {
  const { locale: i18nLocale } = useI18n();
  const recentUsers = [...summary.users]
    .sort(compareNullableDate('lastSeenAt'))
    .slice(0, 6);
  const activeDevices = summary.devices.filter((device) => device.connected);
  return (
    <section className="product-panel grid min-w-0 divide-y divide-[var(--theme-border)] xl:grid-cols-[minmax(0,1fr)_minmax(22rem,0.7fr)] xl:divide-x xl:divide-y-0">
      <Panel title={translate("devices.recentUsers")} detail={translate("devices.lastAuthenticatedRelayActivity")}>
        <div className="divide-y divide-[var(--theme-border)]">
          {recentUsers.map((user) => (
            <div
              className="flex items-center justify-between gap-3 py-3"
              key={user.id}
            >
              <div className="min-w-0">
                <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
                  {user.username}
                </p>
                <p className="truncate text-xs text-[var(--theme-fg-muted)]">
                  {user.email}
                </p>
              </div>
              <div className="text-right text-xs text-[var(--theme-fg-muted)]">
                <p>{formatTimestamp(user.lastSeenAt)}</p>
              </div>
            </div>
          ))}
        </div>
      </Panel>
      <Panel
        title={translate("devices.onlineDevices")}
        detail={translate("devices.devicesWithAnActiveSupervisorTunnel")}
      >
        {activeDevices.length ? (
          <div className="space-y-3">
            {activeDevices.map((device) => (
              <DeviceSummary device={device} key={device.id} />
            ))}
          </div>
        ) : (
          <EmptyState>{translate("devices.noSupervisorsAreConnected")}</EmptyState>
        )}
      </Panel>
    </section>
  );
}

function UsersTable({
  busyKey,
  failure,
  onDeleteUser,
  onResetPassword,
  onUpdateUser,
  users,
}: {
  busyKey: string | null;
  failure: AdminFailure;
  onDeleteUser: (userId: string) => Promise<void>;
  onResetPassword: (userId: string, password: string) => Promise<void>;
  onUpdateUser: (userId: string, enabled: boolean) => void;
  users: RelayAdminSummaryDto['users'];
}) {
  const { locale: i18nLocale } = useI18n();
  const [query, setQuery] = useState('');
  const [userPage, setUserPage] = useState(0);
  const [sort, setSort] = useState<{
    key: UserSortKey;
    direction: SortDirection;
  }>({
    key: 'lastSeenAt',
    direction: 'desc',
  });
  const [resetTarget, setResetTarget] = useState<
    RelayAdminSummaryDto['users'][number] | null
  >(null);
  const [deleteTarget, setDeleteTarget] = useState<
    RelayAdminSummaryDto['users'][number] | null
  >(null);
  const filteredUsers = useMemo(() => {
    const normalized = normalizeSearch(query);
    return users
      .filter((user) => {
        if (!normalized) {
          return true;
        }
        return normalizeSearch(`${user.username} ${user.email}`).includes(
          normalized,
        );
      })
      .sort((left, right) => compareUsers(left, right, sort));
  }, [query, sort, users]);

  function updateSort(key: UserSortKey) {
    setSort((current) => ({
      key,
      direction:
        current.key === key && current.direction === 'desc' ? 'asc' : 'desc',
    }));
  }

  return (
    <>
      <Panel
        title={translate("devices.users")}
        detail={translate("devices.registeredRelayAccountsAdminAccountsAreExcluded")}
      >
        <div className="mb-4 flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <label className="relative block min-w-0 flex-1">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--theme-fg-muted)]" />
            <input
              className="relay-input w-full !pl-9"
              onChange={(event) => { setQuery(event.target.value); setUserPage(0); }}
              placeholder={translate("devices.searchUsernameOrEmail")}
              value={query}
            />
          </label>
          <p className="text-sm text-[var(--theme-fg-muted)]">
            {translate('devices.usersShown', { shown: filteredUsers.length.toLocaleString(getLocale()), total: users.length.toLocaleString(getLocale()) })}</p>
        </div>
        <div className="mb-4 grid grid-cols-2 gap-3 md:hidden">
          <label className="text-xs font-medium text-[var(--theme-fg-muted)]">
            {translate("devices.sortBy")}<select
              className="relay-input mt-1.5 w-full"
              onChange={(event) =>
                setSort((current) => ({
                  ...current,
                  key: event.target.value as UserSortKey,
                }))
              }
              value={sort.key}
            >
              <option value="username">{translate("devices.user_9f8a23")}</option>
              <option value="enabled">{translate("devices.status")}</option>
              <option value="lastSeenAt">{translate("devices.lastUsed")}</option>
              <option value="deviceCount">{translate("devices.devices")}</option>
              <option value="createdAt">{translate("devices.created")}</option>
            </select>
          </label>
          <label className="text-xs font-medium text-[var(--theme-fg-muted)]">
            {translate("devices.direction")}<select
              className="relay-input mt-1.5 w-full"
              onChange={(event) =>
                setSort((current) => ({
                  ...current,
                  direction: event.target.value as SortDirection,
                }))
              }
              value={sort.direction}
            >
              <option value="desc">{translate("devices.descending")}</option>
              <option value="asc">{translate("devices.ascending")}</option>
            </select>
          </label>
        </div>
        <ResponsiveTable minWidth="68rem">
          <thead>
            <tr>
              <SortableTh
                active={sort.key === 'username'}
                direction={sort.direction}
                onClick={() => updateSort('username')}
              >
                {translate("devices.user_9f8a23")}</SortableTh>
              <SortableTh
                active={sort.key === 'enabled'}
                direction={sort.direction}
                onClick={() => updateSort('enabled')}
              >
                {translate("devices.status")}</SortableTh>
              <SortableTh
                active={sort.key === 'lastSeenAt'}
                direction={sort.direction}
                onClick={() => updateSort('lastSeenAt')}
              >
                {translate("devices.lastUsed")}</SortableTh>
              <SortableTh
                active={sort.key === 'deviceCount'}
                direction={sort.direction}
                onClick={() => updateSort('deviceCount')}
              >
                {translate("devices.devices")}</SortableTh>
              <Th>{translate("devices.role")}</Th>
              <Th>{translate("devices.actions")}</Th>
            </tr>
          </thead>
          <tbody>
            {pageOf(filteredUsers, userPage).map((user) => (
              <tr key={user.id}>
                <Td label={translate("devices.user_9f8a23")} strong>
                  {user.username}
                  <div className="text-xs font-normal text-[var(--theme-fg-muted)]">
                    {user.email}
                  </div>
                </Td>
                <Td label={translate("devices.status")}>
                  <StatusPill active={user.enabled}>
                    {user.enabled ? translate("devices.enabled") : translate("devices.disabled")}
                  </StatusPill>
                </Td>
                <Td label={translate("devices.lastUsed")}>{formatTimestamp(user.lastSeenAt)}</Td>
                <Td label={translate("devices.devices")}>{user.deviceCount.toLocaleString(getLocale())}</Td>
                <Td label={translate("devices.role")}>{user.role === 'admin' ? translate('devices.roleAdmin') : translate('devices.roleUser')}</Td>
                <Td label={translate("devices.actions")}>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      className="relay-button-secondary"
                      disabled={busyKey === user.id || user.role === 'admin'}
                      onClick={() => onUpdateUser(user.id, !user.enabled)}
                      type="button"
                    >
                      {user.enabled ? translate("devices.disable") : translate("devices.enable")}
                    </button>
                    <FailureMark at={`user:${user.id}`} failure={failure} />
                    <button
                      className="relay-button-secondary inline-flex items-center gap-2"
                      disabled={
                        busyKey === `reset:${user.id}` || user.role === 'admin'
                      }
                      onClick={() => setResetTarget(user)}
                      type="button"
                    >
                      <KeyRound className="h-4 w-4" />
                      {translate("devices.reset")}</button>
                    <button
                      className="relay-button-secondary inline-flex items-center gap-2 text-[var(--status-danger-fg)]"
                      disabled={
                        busyKey === `delete:${user.id}` || user.role === 'admin'
                      }
                      onClick={() => setDeleteTarget(user)}
                      type="button"
                    >
                      <Trash2 className="h-4 w-4" />
                      {translate("devices.delete")}</button>
                  </div>
                </Td>
              </tr>
            ))}
          </tbody>
        </ResponsiveTable>
        {!filteredUsers.length ? (
          <EmptyState>{translate("devices.noUsersMatchTheCurrentSearch")}</EmptyState>
        ) : null}
        <Pager onPage={setUserPage} page={userPage} total={filteredUsers.length} />
      </Panel>
      {resetTarget ? (
        <PasswordResetDialog
          busy={busyKey === `reset:${resetTarget.id}`}
          onClose={() => setResetTarget(null)}
          onSubmit={async (password) => {
            await onResetPassword(resetTarget.id, password);
            setResetTarget(null);
          }}
          user={resetTarget}
        />
      ) : null}
      {deleteTarget ? (
        <DangerConfirmDialog
          busy={busyKey === `delete:${deleteTarget.id}`}
          confirmLabel={translate("devices.deleteUser")}
          description={translate("devices.deleteTheirDevicesSharesAndAccessHistory", { value1: deleteTarget.username })}
          onClose={() => setDeleteTarget(null)}
          onConfirm={async () => {
            await onDeleteUser(deleteTarget.id);
            setDeleteTarget(null);
          }}
          title={translate("devices.deleteRelayUser")}
        />
      ) : null}
    </>
  );
}

function DevicesPanel({
  devices,
  users,
}: {
  devices: RelayAdminDeviceDto[];
  users: RelayAdminSummaryDto['users'];
}) {
  const { locale: i18nLocale } = useI18n();
  const [ownerId, setOwnerId] = useState('all');
  const [status, setStatus] = useState<'all' | 'online' | 'offline'>('all');
  const [activity, setActivity] = useState<'all' | '24h' | '7d' | '30d'>('all');
  const [sortKey, setSortKey] = useState<DeviceSortKey>('lastActivity');
  const [direction, setDirection] = useState<SortDirection>('desc');
  const [devicePage, setDevicePage] = useState(0);
  const ownerUsers = useMemo(
    () =>
      users.filter((user) =>
        devices.some((device) => device.ownerUserId === user.id),
      ),
    [devices, users],
  );
  const filteredDevices = useMemo(
    () =>
      devices
        .filter((device) => ownerId === 'all' || device.ownerUserId === ownerId)
        .filter(
          (device) =>
            status === 'all' ||
            (status === 'online' ? device.connected : !device.connected),
        )
        .filter(
          (device) =>
            activity === 'all' ||
            isAfterActivityWindow(deviceLastActivity(device), activity),
        )
        .sort((left, right) => compareDevices(left, right, sortKey, direction)),
    [activity, devices, direction, ownerId, sortKey, status],
  );

  return (
    <Panel
      title={translate("devices.devices")}
      detail={translate("devices.supervisorDevicesGroupedByOwnerConnectionState")}
    >
      <div className="mb-4 grid gap-3 md:grid-cols-2 xl:grid-cols-5">
        <label className="block text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.owner")}<select
            className="relay-input mt-2 w-full"
            onChange={(event) => { setOwnerId(event.target.value); setDevicePage(0); }}
            value={ownerId}
          >
            <option value="all">{translate("devices.allUsers")}</option>
            {ownerUsers.map((user) => (
              <option key={user.id} value={user.id}>
                {user.username}
              </option>
            ))}
          </select>
        </label>
        <label className="block text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.status")}<select
            className="relay-input mt-2 w-full"
            onChange={(event) => { setStatus(event.target.value as typeof status); setDevicePage(0); }}
            value={status}
          >
            <option value="all">{translate("devices.allDevices")}</option>
            <option value="online">{translate("devices.online_c3e839")}</option>
            <option value="offline">{translate("devices.offline")}</option>
          </select>
        </label>
        <label className="block text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.lastActivity")}<select
            className="relay-input mt-2 w-full"
            onChange={(event) => {
              setActivity(event.target.value as typeof activity);
              setDevicePage(0);
            }}
            value={activity}
          >
            <option value="all">{translate("devices.anyTime")}</option>
            <option value="24h">{translate("devices.last24Hours")}</option>
            <option value="7d">{translate("devices.last7Days")}</option>
            <option value="30d">{translate("devices.last30Days")}</option>
          </select>
        </label>
        <label className="block text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.sortBy")}<select
            className="relay-input mt-2 w-full"
            onChange={(event) =>
              setSortKey(event.target.value as DeviceSortKey)
            }
            value={sortKey}
          >
            <option value="lastActivity">{translate("devices.lastActivity")}</option>
            <option value="name">{translate("devices.deviceName")}</option>
            <option value="ownerUsername">{translate("devices.owner")}</option>
            <option value="connected">{translate("devices.connection")}</option>
            <option value="createdAt">{translate("devices.created")}</option>
            <option value="workspaceCount">{translate("devices.workspaces")}</option>
            <option value="threadCount">{translate("devices.threads")}</option>
          </select>
        </label>
        <label className="block text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.direction")}<select
            className="relay-input mt-2 w-full"
            onChange={(event) =>
              setDirection(event.target.value as SortDirection)
            }
            value={direction}
          >
            <option value="desc">{translate("devices.descending")}</option>
            <option value="asc">{translate("devices.ascending")}</option>
          </select>
        </label>
      </div>
      <ResponsiveTable minWidth="60rem">
        <thead>
          <tr>
            <Th>{translate("devices.device")}</Th>
            <Th>{translate("devices.owner")}</Th>
            <Th>{translate("devices.status")}</Th>
            <Th>{translate("devices.lastActivity")}</Th>
            <Th>{translate("devices.inventory")}</Th>
          </tr>
        </thead>
        <tbody>
          {pageOf(filteredDevices, devicePage).map((device) => (
            <tr key={device.id}>
              <Td label={translate("devices.device")} strong>
                {device.name}
                <div className="text-xs font-normal text-[var(--theme-fg-muted)]">
                  {device.tokenPreview}
                </div>
              </Td>
              <Td label={translate("devices.owner")}>
                <span className="font-medium text-[var(--theme-fg-soft)]">
                  {device.ownerUsername}
                </span>
                <div className="text-xs text-[var(--theme-fg-muted)]">
                  {device.ownerEmail}
                </div>
              </Td>
              <Td label={translate("devices.status")}>
                <StatusPill active={device.connected}>
                  {device.connected ? translate("devices.online_c3e839") : translate("devices.offline")}
                </StatusPill>
              </Td>
              <Td label={translate("devices.lastActivity")}>
                {/* Only the tunnel says when a device was last active; an offline
                    device's last heartbeat is not kept, so it is not guessed. */}
                {device.connected ? translate('devices.heartbeatAt', { time: formatTimestamp(device.lastHeartbeatAt) }) : translate('devices.noHeartbeatSinceRestart')}
                <div className="text-xs text-[var(--theme-fg-muted)]">
                  {translate("devices.created_21c508")} {formatTimestamp(device.createdAt)}
                </div>
              </Td>
              <Td label={translate("devices.inventory")}>
                <span>{translate('devices.threadsCount', { count: (device.threadCount ?? 0).toLocaleString(getLocale()) })}</span>
                <span className="mx-2 text-[var(--theme-fg-muted)]">·</span>
                <span>{translate('devices.workspacesCount', { count: (device.workspaceCount ?? 0).toLocaleString(getLocale()) })}</span>
                <div className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                  {translate('devices.agentThreadsCount', { count: (device.agentThreadCount ?? 0).toLocaleString(getLocale()) })}
                </div>
              </Td>
            </tr>
          ))}
        </tbody>
      </ResponsiveTable>
      {!filteredDevices.length ? (
        <EmptyState>{translate("devices.noDevicesMatchTheSelectedFilters")}</EmptyState>
      ) : null}
      <Pager onPage={setDevicePage} page={devicePage} total={filteredDevices.length} />
    </Panel>
  );
}

type AdminShareRow = {
  id: string;
  scope: 'thread' | 'workspace' | 'device';
  ownerUsername: string;
  targetUsername: string;
  deviceName: string;
  workspaceLabel: string | null;
  threadAccess: RelaySessionShareDto['threadAccess'];
  workspaceAccess: RelaySessionShareDto['workspaceAccess'];
  lastAccessedAt: string | null;
  expiresAt: string | null;
};

function SharesTable({ grants, shares }: { grants: RelayAccessGrantDto[]; shares: RelaySessionShareDto[] }) {
  useI18n();
  const [page, setPage] = useState(0);
  // Thread shares and grants are one list: both are active shares of something.
  // Thread titles come from conversation content and are not shown to admins.
  const rows: AdminShareRow[] = [
    ...shares.map((share) => ({ ...share, scope: 'thread' as const })),
    ...grants,
  ].sort((left, right) => compareDateValues(right.lastAccessedAt, left.lastAccessedAt));
  return (
    <Panel
      title={translate("devices.shareRelationships")}
      detail={translate("devices.threadGrantsBetweenRelayUsersRevokedGrants")}
    >
      <ResponsiveTable minWidth="56rem">
        <thead>
          <tr>
            <Th>{translate("devices.scope")}</Th>
            <Th>{translate("devices.owner")}</Th>
            <Th>{translate("devices.recipient")}</Th>
            <Th>{translate("devices.device")}</Th>
            <Th>{translate("devices.permissions")}</Th>
            <Th>{translate("devices.lastAccess")}</Th>
            <Th>{translate("devices.status")}</Th>
          </tr>
        </thead>
        <tbody>
          {pageOf(rows, page).map((share) => (
            <tr key={share.id}>
              <Td label={translate("devices.scope")} strong>{scopeLabel(share.scope)}</Td>
              <Td label={translate("devices.owner")}>{share.ownerUsername}</Td>
              <Td label={translate("devices.recipient")}>{share.targetUsername}</Td>
              <Td label={translate("devices.device")}>
                {share.deviceName}
                {share.workspaceLabel ? (
                  <div className="text-xs text-[var(--theme-fg-muted)]">{share.workspaceLabel}</div>
                ) : null}
              </Td>
              <Td label={translate("devices.permissions")}>
                {share.threadAccess === 'read' ? translate('devices.accessRead') : translate('devices.accessControl')} /{' '}
                {workspaceAccessLabel(share.workspaceAccess)}
              </Td>
              <Td label={translate("devices.lastAccess")}>{formatTimestamp(share.lastAccessedAt)}</Td>
              <Td label={translate("devices.status")}>
                {share.expiresAt && share.expiresAt <= new Date().toISOString()
                  ? translate("devices.expired")
                  : translate("devices.active")}
              </Td>
            </tr>
          ))}
        </tbody>
      </ResponsiveTable>
      <Pager onPage={setPage} page={page} total={rows.length} />
    </Panel>
  );
}

function scopeLabel(scope: AdminShareRow['scope']) {
  return scope === 'device'
    ? translate('devices.scopeDevice')
    : scope === 'workspace'
      ? translate('devices.scopeWorkspace')
      : translate('devices.scopeThread');
}

const PAGE_SIZE = 50;

function pageOf<T>(items: T[], page: number) {
  return items.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
}

function Pager({ onPage, page, total }: { onPage: (page: number) => void; page: number; total: number }) {
  useI18n();
  if (total <= PAGE_SIZE) return null;
  const last = Math.ceil(total / PAGE_SIZE) - 1;
  return (
    <nav aria-label={translate('devices.pageRange', { from: page * PAGE_SIZE + 1, to: Math.min(total, (page + 1) * PAGE_SIZE), total })} className="mt-3 flex items-center justify-end gap-2 text-sm text-[var(--theme-fg-muted)]">
      <span>{translate('devices.pageRange', { from: page * PAGE_SIZE + 1, to: Math.min(total, (page + 1) * PAGE_SIZE), total })}</span>
      <button className="relay-button-secondary" disabled={page <= 0} onClick={() => onPage(page - 1)} type="button">
        {translate('devices.previousPage')}
      </button>
      <button className="relay-button-secondary" disabled={page >= last} onClick={() => onPage(page + 1)} type="button">
        {translate('devices.nextPage')}
      </button>
    </nav>
  );
}

function SettingsPanel({
  busy,
  draft,
  failure,
  onChange,
  onReviewRegistration,
  onSave,
  pending,
  reviewBusyKey,
}: {
  busy: boolean;
  draft: RelayRegistrationSettingsDto;
  failure: AdminFailure;
  onChange: (settings: RelayRegistrationSettingsDto) => void;
  onReviewRegistration: (
    requestId: string,
    action: 'approve' | 'reject',
  ) => void;
  onSave: (event: FormEvent<HTMLFormElement>) => void;
  pending: RelayAdminSummaryDto['pendingRegistrations'];
  reviewBusyKey: string | null;
}) {
  const { locale: i18nLocale } = useI18n();
  const settingsLocked = busy || reviewBusyKey !== null;
  return (
    <section className="product-panel grid min-w-0 divide-y divide-[var(--theme-border)] xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] xl:divide-x xl:divide-y-0">
      <Panel
        title={translate("devices.registrationSettings")}
        detail={translate("devices.storedInTheRelayDatabaseEnvironmentPassword")}
      >
        <form className="space-y-4" onSubmit={onSave}>
          <Checkbox
            checked={draft.enabled}
            disabled={settingsLocked}
            label={translate("devices.openRegistration")}
            onChange={(enabled) => onChange({ ...draft, enabled })}
          />
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.registrationPassword")}<input
              className="relay-input mt-2 w-full"
              disabled={settingsLocked}
              onChange={(event) =>
                onChange({ ...draft, registrationPassword: event.target.value })
              }
              placeholder={translate("devices.leaveEmptyForNoInvitePassword")}
              value={draft.registrationPassword ?? ''}
            />
          </label>
          <Checkbox
            checked={draft.approvalRequired}
            disabled={settingsLocked}
            label={translate("devices.requireAdminApproval")}
            onChange={(approvalRequired) =>
              onChange({ ...draft, approvalRequired })
            }
          />
          <div className="space-y-3 border-t border-[var(--theme-border)] pt-4">
            <Checkbox checked={draft.googleAuthEnabled} disabled={settingsLocked || !draft.googleAuthAvailable} label={translate("devices.enableGoogleAuthentication")} onChange={(googleAuthEnabled) => onChange({ ...draft, googleAuthEnabled })} />
            <Checkbox checked={draft.githubAuthEnabled} disabled={settingsLocked || !draft.githubAuthAvailable} label={translate("devices.enableGitHubAuthentication")} onChange={(githubAuthEnabled) => onChange({ ...draft, githubAuthEnabled })} />
            <div className="flex items-center gap-2">
              <Checkbox checked={draft.emailVerificationEnabled} disabled={settingsLocked || !draft.emailVerificationAvailable} label={translate("devices.enableEmailVerification")} onChange={(emailVerificationEnabled) => onChange({ ...draft, emailVerificationEnabled })} />
              {!draft.emailVerificationAvailable ? <ErrorMark kind="unavailable" message={translate("devices.configureTheEmailProviderAndVerificationSecret")} /> : null}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              className="relay-button-primary inline-flex items-center gap-2"
              disabled={settingsLocked}
              type="submit"
            >
              <Settings className="h-4 w-4" />
              {busy ? translate("devices.saving") : translate("devices.saveSettings")}
            </button>
            <FailureMark at="settings" failure={failure} />
          </div>
        </form>
      </Panel>

      <Panel
        title={translate("devices.pendingRegistrations")}
        detail={translate("devices.approveCreatesTheUserRejectKeepsAn")}
      >
        {pending.length ? (
          <div className="divide-y divide-[var(--theme-border)]">
            {pending.map((request) => (
              <div
                className="flex flex-col gap-3 py-3 sm:flex-row sm:items-center sm:justify-between"
                key={request.id}
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
                    {request.username}
                  </p>
                  <p className="truncate text-xs text-[var(--theme-fg-muted)]">
                    {request.email} · {formatTimestamp(request.createdAt)}
                  </p>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    className="relay-button-primary inline-flex items-center gap-2"
                    disabled={reviewBusyKey !== null}
                    onClick={() => onReviewRegistration(request.id, 'approve')}
                    type="button"
                  >
                    <Check className="h-4 w-4" />
                    {translate("devices.approve")}</button>
                  <FailureMark at={`approve:${request.id}`} failure={failure} />
                  <button
                    className="relay-button-secondary"
                    disabled={reviewBusyKey !== null}
                    onClick={() => onReviewRegistration(request.id, 'reject')}
                    type="button"
                  >
                    {translate("devices.reject")}</button>
                  <FailureMark at={`reject:${request.id}`} failure={failure} />
                </div>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState>{translate("devices.noPendingApplications")}</EmptyState>
        )}
      </Panel>
    </section>
  );
}

function DeviceSummary({ device }: { device: RelayAdminDeviceDto }) {
  useI18n();
  return (
    <div className="grid gap-2 text-xs text-[var(--theme-fg-muted)] sm:grid-cols-2">
      <p className="truncate text-sm font-medium text-[var(--theme-fg)] sm:col-span-2">{device.name}</p>
      <p>
        {translate("devices.owner_719379")}{' '}
        <span className="text-[var(--theme-fg-soft)]">{device.ownerUsername}</span>
      </p>
      <p>
        {translate("devices.connected")}{' '}
        <span className="text-[var(--theme-fg-soft)]">
          {formatTimestamp(device.connectedAt)}
        </span>
      </p>
      <p>
        {translate("devices.heartbeat_93fd96")}{' '}
        <span className="text-[var(--theme-fg-soft)]">
          {formatTimestamp(device.lastHeartbeatAt)}
        </span>
      </p>
    </div>
  );
}

function Panel({
  aside,
  children,
  detail,
  title,
}: {
  aside?: React.ReactNode;
  children: React.ReactNode;
  detail: string;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <section className="min-w-0 px-4 py-5 sm:px-5">
      <div className="mb-4 flex items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">
            {title}
          </h2>
          <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">{detail}</p>
        </div>
        {aside}
      </div>
      {children}
    </section>
  );
}

function ResponsiveTable({
  children,
  minWidth,
}: {
  children: React.ReactNode;
  minWidth: string;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div
      aria-label={translate("devices.scrollableDataTable")}
      className="admin-responsive-table max-w-full overflow-x-auto overscroll-x-contain rounded-lg border border-[var(--theme-border)]"
      role="region"
      tabIndex={0}
    >
      <table
        className="w-full border-collapse text-left text-sm"
        style={{ minWidth }}
      >
        {children}
      </table>
    </div>
  );
}

function Th({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <th className="border-b border-[var(--theme-border)] py-2 pr-3 text-xs font-semibold uppercase tracking-[0.08em] text-[var(--theme-fg-muted)] first:pl-3">
      {children}
    </th>
  );
}

function SortableTh({
  active,
  children,
  direction,
  onClick,
}: {
  active: boolean;
  children: React.ReactNode;
  direction: SortDirection;
  onClick: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const Icon = !active
    ? ArrowUpDown
    : direction === 'asc'
      ? ArrowUp
      : ArrowDown;
  return (
    <th
      aria-sort={active ? (direction === 'asc' ? 'ascending' : 'descending') : 'none'}
      className="border-b border-[var(--theme-border)] py-2 pr-3 text-left text-xs font-semibold uppercase tracking-[0.08em] text-[var(--theme-fg-muted)] first:pl-3"
    >
      <button
        className={`inline-flex items-center gap-1.5 rounded-md px-1.5 py-1 transition hover:bg-[var(--theme-hover)] hover:text-[var(--theme-fg)] ${
          active ? 'text-[var(--theme-fg)]' : ''
        }`}
        onClick={onClick}
        type="button"
      >
        {children}
        <Icon className="h-3.5 w-3.5" />
      </button>
    </th>
  );
}

function Td({
  children,
  label,
  strong = false,
}: {
  children: React.ReactNode;
  label: string;
  strong?: boolean;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <td
      data-label={label}
      className={`border-b border-[var(--theme-border)] py-3 pr-3 first:pl-3 ${strong ? 'font-medium text-[var(--theme-fg)]' : 'text-[var(--theme-fg-muted)]'}`}
    >
      <span className="sr-only md:hidden">{label}: </span>
      {children}
    </td>
  );
}

function Checkbox({
  checked,
  disabled = false,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <label className={`flex items-center gap-3 text-sm ${disabled ? 'text-[var(--theme-fg-muted)] opacity-60' : 'text-[var(--theme-fg-soft)]'}`}>
      <input
        checked={checked}
        disabled={disabled}
        className="h-4 w-4 accent-[var(--theme-accent)]"
        onChange={(event) => onChange(event.target.checked)}
        type="checkbox"
      />
      {label}
    </label>
  );
}

function StatusPill({
  active,
  children,
}: {
  active: boolean;
  children: React.ReactNode;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <span
      className={`rounded-full border px-2 py-0.5 text-xs ${
        active
          ? 'border-[var(--status-success-border)] bg-[var(--status-success-bg)] text-[var(--status-success-fg)]'
          : 'border-[var(--theme-border)] bg-[var(--theme-surface)] text-[var(--theme-fg-muted)]'
      }`}
    >
      {children}
    </span>
  );
}

function EmptyState({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <p className="product-empty !min-h-24 !p-4 text-sm">
      {children}
    </p>
  );
}

function PasswordResetDialog({
  busy,
  onClose,
  onSubmit,
  user,
}: {
  busy: boolean;
  onClose: () => void;
  onSubmit: (password: string) => Promise<void>;
  user: RelayAdminSummaryDto['users'][number];
}) {
  const { locale: i18nLocale } = useI18n();
  const [password, setPassword] = useState('');
  // A too-short password is a hint of the field; a failed save is a mark after its button.
  const [localError, setLocalError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const titleId = useId();
  const errorId = useId();

  useDialogLifecycle({
    busy,
    containerRef: dialogRef,
    initialFocusRef: inputRef,
    onClose,
    open: true,
  });

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setLocalError(null);
    setSaveError(null);
    // The relay accepts a temporary password of 12 to 1024 characters.
    if (password.length < 12) {
      setLocalError(translate("devices.passwordMustBeAtLeast12Characters"));
      return;
    }
    try {
      await onSubmit(password);
    } catch (caught) {
      setSaveError(errorMessage(caught));
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center px-4 py-8">
      <button
        aria-label={translate("devices.closeResetPasswordDialog")}
        className="ui-overlay-scrim absolute inset-0 backdrop-blur-[2px]"
        disabled={busy}
        onClick={onClose}
        tabIndex={-1}
        type="button"
      />
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="host-dialog relative z-10 w-full max-w-md space-y-5 rounded-lg border p-5 shadow-[var(--theme-shadow)]"
        onSubmit={submit}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <header className="flex items-start justify-between gap-4">
          <div>
            <h2 className="text-lg font-semibold text-[var(--theme-fg)]" id={titleId}>
              {translate("devices.resetPassword")}</h2>
            <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
              {translate("devices.setANewRelayPasswordFor")} {user.username}.
            </p>
          </div>
          <button
            aria-label={translate("devices.closeResetPasswordDialog")}
            className="product-icon-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            <X aria-hidden="true" className="h-4 w-4" />
          </button>
        </header>
        <div>
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.newPassword")}<input
              aria-describedby={localError ? errorId : undefined}
              aria-invalid={localError ? true : undefined}
              autoComplete="new-password"
              className="relay-input mt-2 w-full"
              onChange={(event) => {
                setPassword(event.target.value);
                setLocalError(null);
                setSaveError(null);
              }}
              ref={inputRef}
              required
              type="password"
              value={password}
            />
          </label>
          {localError ? (
            <p className="mt-3 rounded-md bg-[var(--status-danger-bg)] px-3 py-2 text-sm text-[var(--status-danger-fg)]" id={errorId} role="alert">
              {localError}
            </p>
          ) : null}
        </div>
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <button
              className="relay-button-secondary"
              disabled={busy}
              onClick={onClose}
              type="button"
            >
              {translate("devices.cancel")}</button>
            <div className="flex items-center gap-2">
              <button
                className="relay-button-primary inline-flex flex-1 items-center justify-center gap-2 sm:flex-none"
                disabled={busy || password.length < 12}
                type="submit"
              >
                <KeyRound aria-hidden="true" className="h-4 w-4" />
                {busy ? translate("devices.saving") : translate("devices.savePassword")}
              </button>
              {saveError ? <ErrorMark message={saveError} /> : null}
            </div>
          </div>
      </form>
    </div>
  );
}

function DangerConfirmDialog({
  busy,
  confirmLabel,
  description,
  onClose,
  onConfirm,
  title,
}: {
  busy: boolean;
  confirmLabel: string;
  description: string;
  onClose: () => void;
  onConfirm: () => Promise<void>;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const [localError, setLocalError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useDialogLifecycle({
    busy,
    containerRef: dialogRef,
    initialFocusRef: cancelRef,
    onClose,
    open: true,
  });

  async function confirm() {
    setLocalError(null);
    try {
      await onConfirm();
    } catch (caught) {
      setLocalError(errorMessage(caught));
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center px-4 py-8">
      <button
        aria-label={translate("devices.closeConfirmationDialog")}
        className="ui-overlay-scrim absolute inset-0 backdrop-blur-[2px]"
        disabled={busy}
        onClick={onClose}
        tabIndex={-1}
        type="button"
      />
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="host-dialog relative z-10 w-full max-w-md rounded-lg border p-5 shadow-[var(--theme-shadow)]"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex items-start justify-between gap-4">
          <h2 className="text-lg font-semibold text-[var(--theme-fg)]" id={titleId}>
          {title}
          </h2>
          <button
            aria-label={translate("devices.closeConfirmationDialog")}
            className="product-icon-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            <X aria-hidden="true" className="h-4 w-4" />
          </button>
        </div>
        <p className="mt-2 text-sm leading-6 text-[var(--theme-fg-muted)]" id={descriptionId}>
          {description}
        </p>
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <button
            className="relay-button-secondary"
            disabled={busy}
            onClick={onClose}
            ref={cancelRef}
            type="button"
          >
            {translate("devices.cancel")}</button>
          <div className="flex items-center gap-2">
            <button
              className="ui-action-danger inline-flex min-h-11 flex-1 items-center justify-center gap-2 rounded-md px-4 text-sm font-semibold transition disabled:cursor-not-allowed sm:flex-none"
              disabled={busy}
              onClick={() => void confirm()}
              type="button"
            >
              <Trash2 aria-hidden="true" className="h-4 w-4" />
              {busy ? translate("devices.deleting") : confirmLabel}
            </button>
            {localError ? <ErrorMark message={localError} /> : null}
          </div>
        </div>
      </div>
    </div>
  );
}

type AdminFailure = { at: string; message: string } | null;
function FailureMark({ failure, at }: { failure: AdminFailure; at: string }) {
  return failure?.at === at ? <ErrorMark message={failure.message} /> : null;
}

function normalizeSearch(value: string) {
  return value.trim().toLowerCase();
}

function compareUsers(
  left: RelayAdminSummaryDto['users'][number],
  right: RelayAdminSummaryDto['users'][number],
  sort: { key: UserSortKey; direction: SortDirection },
) {
  const direction = sort.direction === 'asc' ? 1 : -1;
  let value = 0;
  if (sort.key === 'username') {
    value = left.username.localeCompare(right.username);
  } else if (sort.key === 'enabled') {
    value = Number(left.enabled) - Number(right.enabled);
  } else if (sort.key === 'lastSeenAt') {
    value = compareDateValues(left.lastSeenAt, right.lastSeenAt);
  } else if (sort.key === 'deviceCount') {
    value = left.deviceCount - right.deviceCount;
  } else {
    value = compareDateValues(left.createdAt, right.createdAt);
  }
  return value * direction || left.username.localeCompare(right.username);
}

function compareDevices(
  left: RelayAdminDeviceDto,
  right: RelayAdminDeviceDto,
  key: DeviceSortKey,
  direction: SortDirection,
) {
  const multiplier = direction === 'asc' ? 1 : -1;
  let value = 0;
  if (key === 'name') {
    value = left.name.localeCompare(right.name);
  } else if (key === 'ownerUsername') {
    value = left.ownerUsername.localeCompare(right.ownerUsername);
  } else if (key === 'connected') {
    value = Number(left.connected) - Number(right.connected);
  } else if (key === 'lastActivity') {
    value = compareDateValues(
      deviceLastActivity(left),
      deviceLastActivity(right),
    );
  } else if (key === 'createdAt') {
    value = compareDateValues(left.createdAt, right.createdAt);
  } else if (key === 'workspaceCount') {
    value = (left.workspaceCount ?? 0) - (right.workspaceCount ?? 0);
  } else {
    value = (left.threadCount ?? 0) - (right.threadCount ?? 0);
  }
  return value * multiplier || left.name.localeCompare(right.name);
}

function compareDateValues(
  left: string | null | undefined,
  right: string | null | undefined,
) {
  const leftTime = Date.parse(left ?? '');
  const rightTime = Date.parse(right ?? '');
  const normalizedLeft = Number.isFinite(leftTime) ? leftTime : -Infinity;
  const normalizedRight = Number.isFinite(rightTime) ? rightTime : -Infinity;
  return normalizedLeft - normalizedRight;
}

// The relay keeps no history for offline devices: never fall back to the
// creation time, which would read as recent activity.
function deviceLastActivity(device: RelayAdminDeviceDto) {
  return device.lastHeartbeatAt ?? device.connectedAt ?? null;
}

function isAfterActivityWindow(
  value: string | null | undefined,
  window: '24h' | '7d' | '30d',
) {
  const timestamp = Date.parse(value ?? '');
  if (!Number.isFinite(timestamp)) {
    return false;
  }
  const hours = window === '24h' ? 24 : window === '7d' ? 24 * 7 : 24 * 30;
  return timestamp >= Date.now() - hours * 60 * 60 * 1000;
}

function replaceAdminUser(
  summary: RelayAdminSummaryDto | null,
  updated: RelayUserDto,
) {
  if (!summary) {
    return summary;
  }
  return {
    ...summary,
    users: summary.users.map((user) =>
      user.id === updated.id
        ? {
            ...user,
            ...updated,
          }
        : user,
    ),
  };
}

function tabLabel(tab: AdminTab) {
  if(tab==='security')return translate("devices.security");
  switch (tab) {
    case 'overview':
      return translate("devices.overview");
    case 'users':
      return translate("devices.users");
    case 'devices':
      return translate("devices.devices");
    case 'shares':
      return translate("devices.shares_d15dcb");
    case 'settings':
      return translate("devices.settings");
  }
}

function workspaceAccessLabel(access: RelaySessionShareDto['workspaceAccess']) {
  switch (access) {
    case 'write':
      return translate("devices.workspaceWrite_fa0c11");
    case 'read':
      return translate("devices.workspaceRead_10156a");
    case 'none':
    default:
      return translate("devices.noWorkspace_d3694f");
  }
}

function compareNullableDate(field: 'lastSeenAt') {
  return (
    left: { [key in typeof field]: string | null },
    right: { [key in typeof field]: string | null },
  ) => Date.parse(right[field] ?? '') - Date.parse(left[field] ?? '');
}

function formatTimestamp(value: string | null | undefined) {
  return value ? new Date(value).toLocaleString(getLocale()) : translate('devices.never');
}

function initials(username: string | null | undefined) {
  const normalized = username?.trim() ?? '';
  if (!normalized) {
    return '??';
  }
  return Array.from(normalized).slice(0, 2).join('').toUpperCase();
}
