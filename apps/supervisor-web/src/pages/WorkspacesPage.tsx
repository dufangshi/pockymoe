import { devicesHref } from '../lib/devicesRoute';
import { WorkspacesFrame, useDesktopWorkspaceLayout } from '../desktop/WorkspacesFrame';
import type { ReactNode } from 'react';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark, PathChip } from '@pockymoe/thread-ui';
import { DeviceEncryptionStatus } from '../components/DeviceEncryptionStatus';
import { DeviceProtectionPrompt } from '../components/DeviceSetupProgress';
import { TourLauncherButton } from '../components/onboarding/TourProvider';
import { useAppShellNav } from '../components/AppShellNavContext';
import { ProductHeader } from '../components/ProductHeader';
import {
  useEffect,
  useMemo,
  useCallback,
  useState,
} from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';
import {
  ChevronDown,
  FileInput,
  LayoutDashboard,
  Pin,
  Plus,
  RefreshCw,
} from 'lucide-react';

import type { RuntimeConfigDto, WorkspaceDto } from '@pockymoe/shared';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { DeviceStateMark } from '../components/DeviceStateMark';
import { IsolationMark } from '../components/managed/IsolationMark';
import { ManagedObjectMenu, useManagedObjectMenu, type ManagedAction } from '../components/managed/ManagedObjectMenu';
import { workspaceActions } from '../components/managed/workspaceActions';
import { LongTextDialog } from '../components/LongTextDialog';
import { RenameDialog } from '../components/RenameDialog';
import { WorkspaceShareDialog } from '../components/WorkspaceShareDialog';
import {
  ApiError,
  deleteWorkspace,
  fetchRelayPortal,
  fetchRuntimeConfig,
  fetchStorageOverview,
  fetchWorkspace,
  fetchWorkspaces,
  readSelectedRelayDeviceId,
  relayModeActive,
  setWorkspaceIsolation,
  updateWorkspace,
  updateWorkspaceFavorite,
} from '../lib/api';
import {
  currentNewThreadHref,
  currentRelayScopedPath,
  currentThreadsHref,
  relayDeviceIdFromPath,
} from '../lib/relayRoutes';
import { isTourDevice, onTourRoute } from '../lib/tourSandbox/tourDevice';

function sortWorkspaces(left: WorkspaceDto, right: WorkspaceDto) {
  if (left.isFavorite !== right.isFavorite) {
    return left.isFavorite ? -1 : 1;
  }
  return Date.parse(right.lastOpenedAt ?? right.createdAt) - Date.parse(left.lastOpenedAt ?? left.createdAt);
}

function errorText(caught: unknown, fallback: string) {
  return caught instanceof Error ? caught.message : fallback;
}

function lastOpenedLabel(value: string | null) {
  if (!value) {
    return translate("files.notOpenedYet");
  }

  return translate("files.opened", { value1: new Date(value).toLocaleString(getLocale()) });
}

/** The workspaces of this device shared with this account, from the relay's grants. */
async function sharedWorkspaces(deviceId: string): Promise<WorkspaceDto[] | null> {
  const portal = await fetchRelayPortal();
  const ids = [...new Set((portal.sharedThreadsWithMe ?? [])
    .filter((grant) => grant.deviceId === deviceId && grant.scope === 'workspace' && grant.workspaceId)
    .map((grant) => grant.workspaceId!))];
  if (!ids.length) return null;
  const records = await Promise.allSettled(ids.map((id) => fetchWorkspace(id)));
  return records.flatMap((record) => (record.status === 'fulfilled' ? [record.value] : []));
}

export function WorkspacesPage() {
  const { locale: i18nLocale } = useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const relayDeviceId = relayDeviceIdFromPath(location.pathname);
  const deviceId = relayDeviceId ?? (relayModeActive() ? readSelectedRelayDeviceId() : null);
  const shellNav = useAppShellNav();
  // The desktop frame lists the account's own devices; the tour's sample device
  // keeps the plain page, which its in-page answers fill (docs/guided-tour.md).
  const desktopLayout = useDesktopWorkspaceLayout() && !isTourDevice(relayDeviceId);
  const [workspaces, setWorkspaces] = useState<WorkspaceDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const reloadAfterProtection = useCallback(() => { setError(null); setLoadAttempt((attempt) => attempt + 1); }, []);
  const [runtimeConfig, setRuntimeConfig] = useState<RuntimeConfigDto | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  // Why the list or an action failed, as a mark after its control (`at`):
  // 'load', 'rename', `favorite:<id>`, `isolation:<id>`.
  const [error, setError] = useState<{ at: string; message: string; code?: string | undefined } | null>(null);
  const [deviceName, setDeviceName] = useState<{ deviceId: string | null; name: string; owned: boolean; connected?: boolean | undefined } | null>(null);
  useEffect(() => {
    let active = true;
    // The tour's sample device names itself through its own relay answers.
    const loadName = relayModeActive() || onTourRoute()
      ? fetchRelayPortal().then(portal => {
        const owned = portal.devices.find(device => device.id === deviceId);
        return {
          name: owned?.name ?? portal.sharedDevicesWithMe?.find(grant => grant.deviceId === deviceId)?.deviceName ?? '',
          owned: Boolean(owned), connected: owned?.connected,
        };
      })
      // This reads the saved hostname and cached scan, without starting a disk scan.
      : fetchStorageOverview().then(overview => ({ name: overview.deviceName, owned: false }));
    void loadName.then(({ name, owned, ...presence }) => {
      if (active) setDeviceName({ deviceId, name, owned, ...presence });
    }).catch(() => { /* An unavailable name must not block the workspace list. */ });
    return () => { active = false; };
  }, [deviceId]);
  // Only the device's owner shares its workspaces, through the relay.
  const shareDeviceId = deviceId && deviceName?.deviceId === deviceId && deviceName.owned ? deviceId : null;
  const [sharingWorkspace, setSharingWorkspace] = useState<WorkspaceDto | null>(null);
  const [editingWorkspaceId, setEditingWorkspaceId] = useState<string | null>(null);
  const [draftLabel, setDraftLabel] = useState('');
  const [savingWorkspaceId, setSavingWorkspaceId] = useState<string | null>(null);
  const [favoriteWorkspaceId, setFavoriteWorkspaceId] = useState<string | null>(null);
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [deletingWorkspace, setDeletingWorkspace] = useState<WorkspaceDto | null>(null);
  const [deletingWorkspaceBusy, setDeletingWorkspaceBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // A recipient of workspace shares sees those workspaces only, without the owner's actions.
  const [sharedOnly, setSharedOnly] = useState(false);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      setLoading(true);
      setError(null);
      const [workspaceResult, runtimeResult] = await Promise.allSettled([
        fetchWorkspaces(),
        fetchRuntimeConfig(),
      ]);
      if (cancelled) {
        return;
      }
      setLoading(false);
      const shared = workspaceResult.status === 'rejected' && relayDeviceId
        && workspaceResult.reason instanceof ApiError && [401, 403].includes(workspaceResult.reason.statusCode)
        ? await sharedWorkspaces(relayDeviceId).catch(() => null)
        : null;
      if (cancelled) {
        return;
      }
      setSharedOnly(Boolean(shared));
      if (shared) {
        setWorkspaces(shared);
        setError(null);
      } else if (workspaceResult.status === 'fulfilled') {
        setWorkspaces(workspaceResult.value);
        setError(null);
      } else {
        setError({ at: 'load', message: errorText(workspaceResult.reason, translate("files.unableToLoadWorkspaces")), code: (workspaceResult.reason as { code?: string } | null)?.code });
      }
      if (runtimeResult.status === 'fulfilled') {
        setRuntimeConfig(runtimeResult.value);
        setRuntimeError(null);
      } else {
        setRuntimeError(errorText(runtimeResult.reason, translate("files.unableToLoadSupervisorConfig")));
      }
    };

    void load();
    return () => {
      cancelled = true;
    };
  }, [loadAttempt, relayDeviceId]);

  const sortedWorkspaces = useMemo(() => [...workspaces].sort(sortWorkspaces), [workspaces]);

  async function handleFavorite(workspace: WorkspaceDto) {
    if (favoriteWorkspaceId) {
      return;
    }

    const nextFavorite = !workspace.isFavorite;
    setFavoriteWorkspaceId(workspace.id);
    setError(null);
    setWorkspaces((current) =>
      current.map((item) => (item.id === workspace.id ? { ...item, isFavorite: nextFavorite } : item)),
    );
    try {
      const updated = await updateWorkspaceFavorite(workspace.id, { isFavorite: nextFavorite });
      setWorkspaces((current) => current.map((item) => (item.id === updated.id ? updated : item)));
    } catch (caught) {
      setWorkspaces((current) => current.map((item) => (item.id === workspace.id ? workspace : item)));
      setError({ at: `favorite:${workspace.id}`, message: errorText(caught, translate("files.unableToUpdateWorkspace")) });
    } finally {
      setFavoriteWorkspaceId(null);
    }
  }

  async function handleIsolation(workspace: WorkspaceDto) {
    const isolated = !workspace.agentIsolated;
    setError(null);
    setWorkspaces((current) =>
      current.map((item) => (item.id === workspace.id ? { ...item, agentIsolated: isolated } : item)),
    );
    try {
      const updated = await setWorkspaceIsolation(workspace.id, isolated);
      setWorkspaces((current) => current.map((item) => (item.id === updated.id ? updated : item)));
    } catch (caught) {
      setWorkspaces((current) => current.map((item) => (item.id === workspace.id ? workspace : item)));
      setError({ at: `isolation:${workspace.id}`, message: translate('files.isolationFailed', { value1: errorText(caught, translate('files.unableToUpdateWorkspace')) }) });
    }
  }

  async function handleRenameWorkspace(workspaceId: string) {
    const label = draftLabel.trim();
    if (!label) {
      return;
    }
    setSavingWorkspaceId(workspaceId);
    setError(null);
    try {
      const updated = await updateWorkspace(workspaceId, { label });
      setWorkspaces((current) => current.map((item) => (item.id === updated.id ? updated : item)));
      setEditingWorkspaceId(null);
      setDraftLabel('');
    } catch (caught) {
      setError({ at: 'rename', message: errorText(caught, translate("files.unableToRenameWorkspace")) });
    } finally {
      setSavingWorkspaceId(null);
    }
  }

  async function handleDeleteWorkspace() {
    if (!deletingWorkspace) {
      return;
    }
    setDeletingWorkspaceBusy(true);
    setDeleteError(null);
    try {
      await deleteWorkspace(deletingWorkspace.id, {
        confirmWorkspaceId: deletingWorkspace.id,
        confirmLabel: deletingWorkspace.label,
      });
      setWorkspaces((current) => current.filter((item) => item.id !== deletingWorkspace.id));
      setDeletingWorkspace(null);
    } catch (caught) {
      setDeleteError(errorText(caught, translate("files.unableToDeleteWorkspace")));
    } finally {
      setDeletingWorkspaceBusy(false);
    }
  }

  function beginRename(workspace: WorkspaceDto) {
    setError(null);
    setEditingWorkspaceId(workspace.id);
    setDraftLabel(workspace.label);
  }

  function beginDelete(workspace: WorkspaceDto) {
    setDeleteError(null);
    setDeletingWorkspace(workspace);
  }

  // A failed runtime read is a short status; its reason is behind a mark.
  const runtimeSummary = runtimeConfig
    ? runtimeConfig.workspaceRoot
    : runtimeError ? translate("workbench.runtimeNotAvailable") : translate("files.checkingRuntime");
  const rowMark = (at: string) => error?.at === at ? <ErrorMark message={error.message} /> : null;

  const supervisorDetails = (
      <details className="group border-y border-[var(--theme-border)]">
        <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 py-2.5 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-accent-ring)] [&::-webkit-details-marker]:hidden">
          {/* This Supervisor answered: the one device connection mark (§6). A read that failed says why on
              its own mark below and claims nothing about the connection; the slot stays either way. */}
          <span className={`inline-flex shrink-0 ${runtimeConfig ? '' : 'invisible'}`} aria-hidden={runtimeConfig ? undefined : true}>
            <DeviceStateMark showLabel={false} state="online" />
          </span>
          <span className="shrink-0 font-medium text-[var(--theme-fg)]">Supervisor</span>
          <span className="flex min-w-0 flex-1 items-center gap-1.5">
            {runtimeConfig ? <PathChip path={runtimeSummary} className="workspace-path-chip" /> : (
              <span className="min-w-0 truncate font-mono text-xs text-[var(--theme-fg-muted)]" title={runtimeSummary}>
                {runtimeSummary}
              </span>
            )}
            {runtimeError ? <ErrorMark message={runtimeError} /> : null}
          </span>
          <span className="hidden shrink-0 text-xs tabular-nums text-[var(--theme-fg-muted)] sm:inline">
            {workspaces.length} {workspaces.length === 1 ? translate("files.workspace_a279fe") : translate("files.workspaces_e6f0a0")}
          </span>
          <ChevronDown
            aria-hidden="true"
            className="h-4 w-4 shrink-0 text-[var(--theme-fg-muted)] transition-transform duration-200 group-open:rotate-180"
          />
        </summary>
        <dl className="grid gap-x-8 gap-y-3 border-t border-[var(--theme-border)] py-3 sm:grid-cols-2 lg:grid-cols-3">
          <RuntimeFact label={translate("files.workspaceRoot")} value={runtimeSummary} path={Boolean(runtimeConfig)} />
          <RuntimeFact
            label={translate("files.environment")}
            value={
              runtimeConfig
                ? `${runtimeConfig.environment} | ${runtimeConfig.host}:${runtimeConfig.port}`
                : translate("workbench.runtimeNotAvailable")
            }
          />
          <RuntimeFact
            label={translate("files.version")}
            value={runtimeConfig ? `${runtimeConfig.appName} ${runtimeConfig.appVersion}` : translate("workbench.runtimeNotAvailable")}
          />
        </dl>
      </details>
  );
  const content = (
    <div className={desktopLayout ? "desktop-workspaces-content space-y-6" : "product-page space-y-4"} data-page="workspaces">
      {desktopLayout ? <header className="desktop-workspaces-title"><h1>{translate('files.workspaces')}</h1><button className="product-icon-button" aria-label={translate('desktopDevices.refresh')} title={translate('desktopDevices.refresh')} disabled={loading} onClick={() => { setLoading(true); setLoadAttempt(attempt => attempt + 1); }}><RefreshCw size={16} className={loading ? 'request-spin' : undefined} aria-hidden /></button></header> : <ProductHeader title={translate("files.workspaces")} subtitle={deviceName?.deviceId === deviceId ? deviceName.name : undefined} {...(relayDeviceId ? {backHref: devicesHref(), backLabel: translate("files.backToDevices")} : {})} actions={<>
        {!relayDeviceId && <TourLauncherButton showLabel />}
        {shellNav?.openThreadMap && <button type="button" onClick={shellNav.openThreadMap} aria-label={translate('workbench.threadMap')} title={translate('workbench.threadMap')} className="product-icon-button"><LayoutDashboard data-icon="thread-map" size={19} /></button>}
        {!sharedOnly && <Link to={currentRelayScopedPath('/threads/import')} aria-label={translate("files.importSession")} title={translate("files.importSession")} className="product-icon-button"><FileInput data-icon="import" size={19} /></Link>}
        {!sharedOnly && <Link to={currentRelayScopedPath('/workspaces/new')} aria-label={translate("files.addWorkspace")} title={translate("files.addWorkspace")} className="product-icon-button"><Plus data-icon="add" size={20} /></Link>}
      </>} />}

      {/* An offline device opens like any other and says so here, with the device connection mark (§6). */}
      {deviceName?.deviceId === deviceId && deviceName.connected === false ? <p className="flex items-center gap-2 text-sm text-[var(--theme-fg-muted)]"><DeviceStateMark state="offline" label={translate('desktopDevices.offline')} /><ErrorMark kind="unavailable" message={translate('devices.manageOffline')} /></p> : null}
      {!desktopLayout && supervisorDetails}

      {/* A device whose protection is unfinished blocks every request: say the step that unblocks it. */}
      {!loading && error?.at === 'load' && relayDeviceId ? (
        <DeviceProtectionPrompt
          deviceId={relayDeviceId}
          missingCertificate={error.code === 'accountCertificateMissing'}
          onFinished={reloadAfterProtection}
        />
      ) : null}
      {/* The list could not load: a short label, the reason behind a mark, and a visible Retry. */}
      {!loading && error?.at === 'load' ? (
        <div className="flex min-h-11 flex-wrap items-center gap-3 text-sm">
          <span className="flex items-center gap-1.5 text-[var(--theme-fg-muted)]">
            {translate("files.unableToLoadWorkspaces")}
            {error.message !== translate("files.unableToLoadWorkspaces") ? <ErrorMark message={error.message} /> : null}
          </span>
          {relayDeviceId ? <DeviceEncryptionStatus deviceId={relayDeviceId} /> : null}
          <button
            className="host-secondary-button inline-flex min-h-11 shrink-0 items-center justify-center gap-2 rounded-md border px-3 text-xs font-semibold"
            onClick={() => { setError(null); setLoadAttempt((attempt) => attempt + 1); }}
            type="button"
          >
            <RefreshCw data-icon="refresh" aria-hidden="true" className="h-3.5 w-3.5" />
            {translate("files.retry")}</button>
        </div>
      ) : null}

      {loading ? (
        <div className="product-list" aria-label={translate("files.loadingWorkspaces")} aria-busy="true">
          {[0, 1, 2].map((item) => (
            <div key={item} className="product-row min-h-[5.5rem]">
              <div className="min-w-0 flex-1 space-y-2.5">
                <div className="product-skeleton h-4 w-40 max-w-[65%]" />
                <div className="product-skeleton h-3 w-72 max-w-[85%]" />
              </div>
              <div className="product-skeleton h-9 w-20" />
            </div>
          ))}
        </div>
      ) : null}

      {!loading && error?.at !== 'load' && workspaces.length === 0 ? (
        <section className="product-panel">
          <div className="product-empty">
            <div>
              <h2 className="text-lg font-semibold text-[var(--theme-fg)]">{translate("files.noWorkspacesYet")}</h2>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6">
                {translate("files.addAFolderOnThisDeviceConnect")}</p>
              <Link
                to={currentRelayScopedPath('/workspaces/new')}
                className="ui-action-primary mt-5 inline-flex h-11 items-center gap-2 rounded-md px-4 text-sm font-medium transition"
              >
                <Plus data-icon="add" aria-hidden="true" className="h-4 w-4" />
                {translate("files.addWorkspace")}</Link>
            </div>
          </div>
        </section>
      ) : null}

      {!loading && sortedWorkspaces.length > 0 ? (
        <section aria-label={translate("files.workspaceRegistry")}>
          <div className="product-list !overflow-visible">
            {sortedWorkspaces.map((workspace) => (
              <WorkspaceRow
                key={workspace.id}
                workspace={workspace}
                readOnly={sharedOnly}
                favoriteBusy={favoriteWorkspaceId === workspace.id}
                onFavorite={() => void handleFavorite(workspace)}
                favoriteMark={rowMark(`favorite:${workspace.id}`)}
                isolationMark={rowMark(`isolation:${workspace.id}`)}
                menuItems={workspaceActions(workspace, {
                  navigate,
                  openHref: currentThreadsHref(workspace.id),
                  newThreadHref: currentNewThreadHref(workspace.id),
                  canShare: Boolean(shareDeviceId),
                  manage: (action) => {
                    if (action === 'rename') beginRename(workspace);
                    else if (action === 'share') setSharingWorkspace(workspace);
                    else if (action === 'isolate') void handleIsolation(workspace);
                    else if (action === 'path') setExpandedPath(workspace.absPath);
                    else beginDelete(workspace);
                  },
                })}
              />
            ))}
          </div>
        </section>
      ) : null}

      {desktopLayout && supervisorDetails}
      <RenameDialog
        open={editingWorkspaceId !== null}
        title={translate("files.renameWorkspace")}
        label={translate("files.workspaceLabel")}
        value={draftLabel}
        busy={savingWorkspaceId !== null}
        error={editingWorkspaceId && error?.at === 'rename' ? error.message : null}
        onChange={setDraftLabel}
        onCancel={() => {
          setEditingWorkspaceId(null);
          setDraftLabel('');
          setError(null);
        }}
        onSubmit={() => (editingWorkspaceId ? handleRenameWorkspace(editingWorkspaceId) : undefined)}
      />
      {shareDeviceId ? (
        <WorkspaceShareDialog deviceId={shareDeviceId} workspace={sharingWorkspace} onClose={() => setSharingWorkspace(null)} />
      ) : null}
      <LongTextDialog
        open={expandedPath !== null}
        title={translate("files.workspacePath")}
        text={expandedPath ?? ''}
        onClose={() => setExpandedPath(null)}
      />
      <ConfirmDialog
        open={deletingWorkspace !== null}
        title={translate("files.removeWorkspaceTitle")}
        description={
          deletingWorkspace
            // The failure is the mark after the confirm button, not a sentence here.
            ? translate("files.removeAndItsThreadsFromThisSupervisor", { value1: deletingWorkspace.label, value2: '' })
            : ''
        }
        confirmLabel={translate("files.removeWorkspaceConfirm")}
        busyLabel={translate("files.removingWorkspace")}
        busy={deletingWorkspaceBusy}
        error={deleteError}
        onCancel={() => {
          if (!deletingWorkspaceBusy) {
            setDeletingWorkspace(null);
            setDeleteError(null);
          }
        }}
        onConfirm={() => void handleDeleteWorkspace()}
      />
    </div>
  );
  return desktopLayout ? <WorkspacesFrame deviceId={deviceId} sharedOnly={sharedOnly}>{content}</WorkspacesFrame> : content;
}

/** One workspace: the row opens it; its ⋯ menu, a right click and a long press manage it (§3). */
function WorkspaceRow({ workspace, readOnly, favoriteBusy, onFavorite, favoriteMark, isolationMark, menuItems }: {
  workspace: WorkspaceDto;
  /** A recipient of a share sees the workspace without the owner's actions. */
  readOnly: boolean;
  favoriteBusy: boolean;
  onFavorite: () => void;
  favoriteMark: ReactNode;
  isolationMark: ReactNode;
  /** The workspace's actions, in the order shared with a device's (workspaceActions). */
  menuItems: ManagedAction[];
}) {
  useI18n();
  const menu = useManagedObjectMenu();
  return (
    <article
      aria-label={workspace.label}
      className="product-row relative !grid min-w-0 grid-cols-[minmax(0,1fr)_auto] !gap-0 !p-0"
      {...(readOnly ? {} : menu.rowProps)}
    >
      {/* The name is a link stretched over the card; the path chip and the isolation
          mark sit above it, so they can show the whole path and what isolation means (§2, §8). */}
      <div className="relative min-w-0 px-4 py-3.5 sm:px-5">
        <div className="flex min-w-0 items-center gap-2">
          <Link
            to={currentThreadsHref(workspace.id)}
            className="group block min-w-0 after:absolute after:inset-0 after:content-[''] focus-visible:outline-none focus-visible:after:z-10 focus-visible:after:ring-2 focus-visible:after:ring-inset focus-visible:after:ring-[var(--theme-accent-ring)]"
          >
            <span className="block truncate text-sm font-semibold text-[var(--theme-fg)] sm:text-base">
              {workspace.label}
            </span>
          </Link>
          {workspace.agentIsolated ? <span className="relative z-20 inline-flex shrink-0"><IsolationMark kind="workspace" /></span> : null}
        </div>
        <PathChip path={workspace.absPath} className="workspace-path-chip relative z-20 mt-1" />
        <time
          dateTime={workspace.lastOpenedAt ?? undefined}
          className="mt-1 block text-xs text-[var(--theme-fg-muted)]"
        >
          {lastOpenedLabel(workspace.lastOpenedAt)}
        </time>
      </div>

      {readOnly ? null : <div className="flex items-center gap-1 pr-2 sm:pr-3">
        <button
          type="button"
          disabled={favoriteBusy}
          onClick={onFavorite}
          aria-label={workspace.isFavorite ? translate("files.unpin", { value1: workspace.label }) : translate("files.pin", { value1: workspace.label })}
          title={workspace.isFavorite ? translate("files.unpinWorkspace") : translate("files.pinWorkspace")}
          className={`product-icon-button disabled:cursor-wait disabled:opacity-60 ${
            workspace.isFavorite
              ? 'text-[var(--status-warning-fg)] hover:bg-[var(--status-warning-bg)]'
              : ''
          }`}
        >
          <Pin
            data-icon="pin"
            aria-hidden="true"
            className={`h-4 w-4 ${workspace.isFavorite ? 'rotate-[18deg] fill-current' : 'rotate-[8deg]'}`}
          />
        </button>
        {favoriteMark}
        <ManagedObjectMenu ref={menu.ref} name={workspace.label} items={menuItems} className="product-icon-button" />
        {isolationMark}
      </div>}
    </article>
  );
}

function RuntimeFact({ label, value, path = false }: { label: string; value: string; /** A path is a path chip wherever it shows (§2). */ path?: boolean }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div className="min-w-0">
      <dt className="text-xs text-[var(--theme-fg-muted)]">{label}</dt>
      <dd className="mt-0.5 break-words font-mono text-xs leading-5 text-[var(--theme-fg)]">{path ? <PathChip path={value} className="workspace-path-chip" /> : value}</dd>
    </div>
  );
}
