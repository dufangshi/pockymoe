import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { DeviceEncryptionStatus } from '../components/DeviceEncryptionStatus';
import { TourLauncherButton } from '../components/onboarding/TourProvider';
import { useAppShellNav } from '../components/AppShellNavContext';
import { ProductHeader } from '../components/ProductHeader';
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { Link, useLocation } from 'react-router-dom';
import {
  ChevronDown,
  Eye,
  FileInput,
  LayoutDashboard,
  Lock,
  LockOpen,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  RefreshCw,
  Trash2,
} from 'lucide-react';

import type { RuntimeConfigDto, WorkspaceDto } from '@pockymoe/shared';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { LongTextDialog } from '../components/LongTextDialog';
import { RenameDialog } from '../components/RenameDialog';
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
  currentRelayScopedPath,
  currentThreadsHref,
  relayDeviceIdFromPath,
} from '../lib/relayRoutes';

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
  const relayDeviceId = relayDeviceIdFromPath(location.pathname);
  const deviceId = relayDeviceId ?? (relayModeActive() ? readSelectedRelayDeviceId() : null);
  const shellNav = useAppShellNav();
  const [workspaces, setWorkspaces] = useState<WorkspaceDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [runtimeConfig, setRuntimeConfig] = useState<RuntimeConfigDto | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deviceName, setDeviceName] = useState<{ deviceId: string | null; name: string } | null>(null);
  useEffect(() => {
    let active = true;
    const loadName = relayModeActive()
      ? fetchRelayPortal().then(portal =>
        portal.devices.find(device => device.id === deviceId)?.name
        ?? portal.sharedDevicesWithMe?.find(grant => grant.deviceId === deviceId)?.deviceName
        ?? '')
      // This reads the saved hostname and cached scan, without starting a disk scan.
      : fetchStorageOverview().then(overview => overview.deviceName);
    void loadName.then(name => {
      if (active) setDeviceName({ deviceId, name });
    }).catch(() => { /* An unavailable name must not block the workspace list. */ });
    return () => { active = false; };
  }, [deviceId]);
  const [editingWorkspaceId, setEditingWorkspaceId] = useState<string | null>(null);
  const [draftLabel, setDraftLabel] = useState('');
  const [savingWorkspaceId, setSavingWorkspaceId] = useState<string | null>(null);
  const [favoriteWorkspaceId, setFavoriteWorkspaceId] = useState<string | null>(null);
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [deletingWorkspace, setDeletingWorkspace] = useState<WorkspaceDto | null>(null);
  const [deletingWorkspaceBusy, setDeletingWorkspaceBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // A recipient of workspace shares sees those workspaces only, without the owner's actions.
  const [sharedOnly, setSharedOnly] = useState(false);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
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
        setError(errorText(workspaceResult.reason, translate("files.unableToLoadWorkspaces")));
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
      setError(errorText(caught, translate("files.unableToUpdateWorkspace")));
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
      setError(translate('files.isolationFailed', { value1: errorText(caught, translate('files.unableToUpdateWorkspace')) }));
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
      setError(errorText(caught, translate("files.unableToRenameWorkspace")));
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
    setOpenMenuId(null);
    setError(null);
    setEditingWorkspaceId(workspace.id);
    setDraftLabel(workspace.label);
  }

  function beginDelete(workspace: WorkspaceDto) {
    setOpenMenuId(null);
    setDeleteError(null);
    setDeletingWorkspace(workspace);
  }

  const runtimeSummary = runtimeConfig
    ? runtimeConfig.workspaceRoot
    : runtimeError ?? translate("files.checkingRuntime");

  return (
    <div className="product-page space-y-4" data-page="workspaces">
      <ProductHeader title={translate("files.workspaces")} subtitle={deviceName?.deviceId === deviceId ? deviceName.name : undefined} {...(relayDeviceId ? {backHref: '/relay-devices', backLabel: translate("files.backToDevices")} : {})} actions={<>
        {!relayDeviceId && <TourLauncherButton showLabel />}
        {shellNav?.openThreadMap && <button type="button" onClick={shellNav.openThreadMap} aria-label={translate('workbench.threadMap')} title={translate('workbench.threadMap')} className="product-icon-button"><LayoutDashboard data-icon="thread-map" size={19} /></button>}
        {!sharedOnly && <Link to={currentRelayScopedPath('/threads/import')} aria-label={translate("files.importSession")} title={translate("files.importSession")} className="product-icon-button"><FileInput data-icon="import" size={19} /></Link>}
        {!sharedOnly && <Link to={currentRelayScopedPath('/workspaces/new')} aria-label={translate("files.addWorkspace")} title={translate("files.addWorkspace")} className="product-icon-button"><Plus data-icon="add" size={20} /></Link>}
      </>} />

      <details className="group border-y border-[var(--theme-border)]">
        <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 py-2.5 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-accent-ring)] [&::-webkit-details-marker]:hidden">
          <span
            aria-hidden="true"
            className={`h-2 w-2 shrink-0 rounded-full ${
              runtimeError
                ? 'bg-[var(--status-warning-fg)]'
                : runtimeConfig
                  ? 'bg-[var(--status-success-fg)]'
                  : 'animate-pulse bg-[var(--theme-fg-muted)]'
            }`}
          />
          <span className="shrink-0 font-medium text-[var(--theme-fg)]">Supervisor</span>
          <span className="min-w-0 flex-1 truncate font-mono text-xs text-[var(--theme-fg-muted)]" title={runtimeSummary}>
            {runtimeSummary}
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
          <RuntimeFact label={translate("files.workspaceRoot")} value={runtimeSummary} />
          <RuntimeFact
            label={translate("files.environment")}
            value={
              runtimeConfig
                ? `${runtimeConfig.environment} | ${runtimeConfig.host}:${runtimeConfig.port}`
                : runtimeError ?? translate("workbench.runtimeNotAvailable")
            }
          />
          <RuntimeFact
            label={translate("files.version")}
            value={runtimeConfig ? `${runtimeConfig.appName} ${runtimeConfig.appVersion}` : translate("workbench.runtimeNotAvailable")}
          />
        </dl>
      </details>

      {error ? (
        <div className="host-error flex min-h-11 flex-col gap-3 rounded-md border px-4 py-3 text-sm sm:flex-row sm:items-center sm:justify-between" role="alert">
          <span>{error}</span>
          {relayDeviceId ? <DeviceEncryptionStatus deviceId={relayDeviceId} /> : null}
          {!editingWorkspaceId ? (
            <button
              className="host-secondary-button inline-flex min-h-11 shrink-0 items-center justify-center gap-2 rounded-md border px-3 text-xs font-semibold"
              onClick={() => setLoadAttempt((attempt) => attempt + 1)}
              type="button"
            >
              <RefreshCw data-icon="refresh" aria-hidden="true" className="h-3.5 w-3.5" />
              {translate("files.retry")}</button>
          ) : null}
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

      {!loading && !error && workspaces.length === 0 ? (
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
            {sortedWorkspaces.map((workspace) => {
              const menuOpen = openMenuId === workspace.id;
              const favoriteBusy = favoriteWorkspaceId === workspace.id;

              return (
                <article
                  key={workspace.id}
                  className={`product-row relative !grid min-w-0 grid-cols-[minmax(0,1fr)_auto] !gap-0 !p-0 ${
                    menuOpen ? 'z-20' : ''
                  }`}
                >
                  <Link
                    to={currentThreadsHref(workspace.id)}
                    className="group min-w-0 px-4 py-3.5 focus-visible:outline-none focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-accent-ring)] sm:px-5"
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-sm font-semibold text-[var(--theme-fg)] sm:text-base">
                        {workspace.label}
                      </span>
                      {workspace.agentIsolated ? (
                        <span
                          className="workspace-isolated-badge inline-flex shrink-0 items-center gap-1 rounded-full border border-[var(--theme-border)] px-1.5 py-0.5 text-[11px] font-medium leading-none text-[var(--theme-fg-muted)]"
                          title={translate('files.workspaceIsolatedHint')}
                        >
                          <Lock data-icon="isolated" aria-hidden="true" className="h-3 w-3" />
                          {translate('files.workspaceIsolated')}
                        </span>
                      ) : null}

                    </span>
                    <span
                      className="mt-1 block truncate font-mono text-xs leading-5 text-[var(--theme-fg-muted)]"
                      title={workspace.absPath}
                    >
                      {workspace.absPath}
                    </span>
                    <time
                      dateTime={workspace.lastOpenedAt ?? undefined}
                      className="mt-1 block text-xs text-[var(--theme-fg-muted)]"
                    >
                      {lastOpenedLabel(workspace.lastOpenedAt)}
                    </time>
                  </Link>

                  {sharedOnly ? null : <div className="flex items-center gap-1 pr-2 sm:pr-3">
                    <button
                      type="button"
                      disabled={favoriteBusy}
                      onClick={() => void handleFavorite(workspace)}
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
                    <WorkspaceActionsMenu
                      workspace={workspace}
                      open={menuOpen}
                      onOpenChange={(nextOpen) => setOpenMenuId(nextOpen ? workspace.id : null)}
                      onViewPath={() => setExpandedPath(workspace.absPath)}
                      onRename={() => beginRename(workspace)}
                      onToggleIsolation={workspace.agentIsolated === undefined ? undefined : () => void handleIsolation(workspace)}
                      onDelete={() => beginDelete(workspace)}
                    />
                  </div>}
                </article>
              );
            })}
          </div>
        </section>
      ) : null}

      <RenameDialog
        open={editingWorkspaceId !== null}
        title={translate("files.renameWorkspace")}
        label={translate("files.workspaceLabel")}
        value={draftLabel}
        busy={savingWorkspaceId !== null}
        error={editingWorkspaceId ? error : null}
        onChange={setDraftLabel}
        onCancel={() => {
          setEditingWorkspaceId(null);
          setDraftLabel('');
          setError(null);
        }}
        onSubmit={() => (editingWorkspaceId ? handleRenameWorkspace(editingWorkspaceId) : undefined)}
      />
      <LongTextDialog
        open={expandedPath !== null}
        title={translate("files.workspacePath")}
        text={expandedPath ?? ''}
        onClose={() => setExpandedPath(null)}
      />
      <ConfirmDialog
        open={deletingWorkspace !== null}
        title={translate("files.deleteWorkspace")}
        description={
          deletingWorkspace
            ? translate("files.removeAndItsThreadsFromThisSupervisor", { value1: deletingWorkspace.label, value2: deleteError ? translate("files.theLastAttemptFailed", { value1: deleteError }) : '' })
            : ''
        }
        confirmLabel={translate("files.deleteWorkspace_bc2cf9")}
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
}

function WorkspaceActionsMenu({
  workspace,
  open,
  onOpenChange,
  onViewPath,
  onRename,
  onToggleIsolation,
  onDelete,
}: {
  workspace: WorkspaceDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onViewPath: () => void;
  onRename: () => void;
  /** Absent when the device's runtime predates agent isolation. */
  onToggleIsolation?: (() => void) | undefined;
  onDelete: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const menuId = `workspace-actions-${workspace.id}`;

  useEffect(() => {
    if (!open) {
      return;
    }

    const focusFirstItem = window.requestAnimationFrame(() => {
      menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    });

    function handlePointerDown(event: PointerEvent) {
      if (!containerRef.current?.contains(event.target as Node)) {
        onOpenChange(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        onOpenChange(false);
        triggerRef.current?.focus();
      }
    }

    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.cancelAnimationFrame(focusFirstItem);
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [onOpenChange, open]);

  function handleMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);

    if (event.key === 'Tab') {
      onOpenChange(false);
      return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key) || items.length === 0) {
      return;
    }

    event.preventDefault();
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? items.length - 1
          : event.key === 'ArrowDown'
            ? (currentIndex + 1) % items.length
            : (currentIndex - 1 + items.length) % items.length;
    items[nextIndex]?.focus();
  }

  function runAction(action: () => void) {
    triggerRef.current?.focus();
    onOpenChange(false);
    action();
  }

  return (
    <div ref={containerRef} className="relative shrink-0">
      <button
        ref={triggerRef}
        type="button"
        aria-label={translate("files.moreActionsFor", { value1: workspace.label })}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        title={translate("files.workspaceActions")}
        onClick={() => onOpenChange(!open)}
        className="product-icon-button"
      >
        <MoreHorizontal data-icon="more" aria-hidden="true" className="h-5 w-5" />
      </button>
      {open ? (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label={translate("files.actionsFor", { value1: workspace.label })}
          onKeyDown={handleMenuKeyDown}
          className="absolute right-0 top-[calc(100%+0.375rem)] z-30 w-52 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1.5 shadow-[var(--theme-shadow)]"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => runAction(onViewPath)}
            className="flex min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:bg-[var(--theme-hover)]"
          >
            <Eye data-icon="view-path" aria-hidden="true" className="h-4 w-4 text-[var(--theme-fg-muted)]" />
            {translate("files.viewFullPath")}</button>
          <button
            type="button"
            role="menuitem"
            onClick={() => runAction(onRename)}
            className="flex min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:bg-[var(--theme-hover)]"
          >
            <Pencil data-icon="rename" aria-hidden="true" className="h-4 w-4 text-[var(--theme-fg-muted)]" />
            {translate("files.rename_d3f4cb")}</button>
          {onToggleIsolation ? (
            <button
              type="button"
              role="menuitem"
              onClick={() => runAction(onToggleIsolation)}
              title={translate('files.workspaceIsolatedHint')}
              className="flex min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:bg-[var(--theme-hover)]"
            >
              {workspace.agentIsolated ? (
                <LockOpen data-icon="unisolate" aria-hidden="true" className="h-4 w-4 text-[var(--theme-fg-muted)]" />
              ) : (
                <Lock data-icon="isolate" aria-hidden="true" className="h-4 w-4 text-[var(--theme-fg-muted)]" />
              )}
              {translate(workspace.agentIsolated ? 'files.stopIsolatingWorkspace' : 'files.isolateWorkspace')}
            </button>
          ) : null}
          <div className="my-1 border-t border-[var(--theme-border)]" />
          <button
            type="button"
            role="menuitem"
            onClick={() => runAction(onDelete)}
            className="flex min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm text-[var(--status-danger-fg)] transition hover:bg-[var(--status-danger-bg)] focus-visible:outline-none focus-visible:bg-[var(--status-danger-bg)]"
          >
            <Trash2 data-icon="delete" aria-hidden="true" className="h-4 w-4" />
            {translate("files.deleteWorkspace_bc2cf9")}</button>
        </div>
      ) : null}
    </div>
  );
}

function RuntimeFact({ label, value }: { label: string; value: string }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div className="min-w-0">
      <dt className="text-xs text-[var(--theme-fg-muted)]">{label}</dt>
      <dd className="mt-0.5 break-words font-mono text-xs leading-5 text-[var(--theme-fg)]">{value}</dd>
    </div>
  );
}
