import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import {
  CloudDownload,
  FlaskConical,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
} from 'lucide-react';
import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import {
  connectionErrorMessage,
  connectionState,
  protocolShort,
  servedProtocols,
  declaredContextWindow,
  tokenCount,
  remoteSources,
  syncSources,
  type ConnectionSource,
  type ConnectionState,
  type ConnectionSummary,
} from '../lib/connections';
import type { ConnectionCatalogStore } from '../lib/useConnectionCatalog';
import { ConnectionDialog } from './ConnectionDialog';
import { FormDialog } from './FormDialog';

export const button =
  'host-secondary-button inline-flex min-h-10 shrink-0 items-center justify-center gap-1 rounded-lg border px-2.5 text-xs font-medium whitespace-nowrap disabled:opacity-40 sm:min-h-9';
export const iconButton =
  'host-secondary-button inline-flex min-h-10 min-w-10 shrink-0 items-center justify-center rounded-lg border disabled:opacity-40 sm:min-h-9 sm:min-w-9';
const warnPill =
  'bg-[var(--status-warning-bg,var(--theme-hover))] text-[var(--status-warning-fg,var(--theme-fg))]';
const stateOrder: Record<ConnectionState, number> = {
  local: 0,
  conflict: 1,
  remote: 2,
  offline: 3,
};

type Usage = { harness: string; pending: boolean };

/**
 * The account's connections as seen from this device, independent of any harness.
 * Sync copies a remote connection here; applying to a harness happens elsewhere.
 */
export function ConnectionCatalog({
  apiRoot,
  store,
  harnessNames = {},
  onRefresh,
}: {
  apiRoot: string;
  store: ConnectionCatalogStore;
  /** Display names for harness ids in the catalog's bindings. */
  harnessNames?: Record<string, string>;
  onRefresh?: () => void;
}) {
  useI18n();
  const {
    api,
    catalog,
    loading,
    error,
    reload,
    sync,
    syncing,
    syncErrors,
    upsert,
  } = store;
  const [editing, setEditing] = useState<ConnectionSummary | 'new' | null>(
    null,
  );
  const [choosing, setChoosing] = useState<ConnectionSummary | null>(null);
  const [deleting, setDeleting] = useState<ConnectionSummary | null>(null);
  // A result shows where it was asked for: on its row. Only a notice with no row
  // left (a removal) shows at the top of the list.
  const [notice, setNotice] = useState('');
  const [rowNotice, setRowNotice] = useState<Record<string, string>>({});
  const [rowError, setRowError] = useState<Record<string, string>>({});
  const noteRow = (id: string, text: string) => {
    setNotice('');
    setRowNotice((current) => ({ ...current, [id]: text }));
  };
  const clearRow = (id: string) => {
    setNotice('');
    setRowNotice(({ [id]: _, ...rest }) => rest);
  };
  const [testing, setTesting] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  // Reloads after a save, sync or removal keep the list quietly when they fail;
  // a Refresh the reader asked for gets a mark until a reload succeeds.
  const [refreshAsked, setRefreshAsked] = useState(false);
  useEffect(() => {
    if (!loading && !error) setRefreshAsked(false);
  }, [loading, error]);
  // An older Supervisor has no catalog: Add stays greyed, with the reason on a mark.
  const unsupported =
    !catalog && error === translate('settings.connUpdateSupervisor');
  const deviceId = catalog?.deviceId ?? null;
  const all = catalog?.connections ?? [];
  // Which harnesses use each local connection, and whether they still run an
  // older applied revision (saving or syncing never changes a harness).
  const usage = (c: ConnectionSummary): Usage[] =>
    c.local
      ? Object.entries(catalog?.bindings ?? {})
          .filter(([, b]) => b.connectionId === c.id)
          .map(([id, b]) => ({
            harness: harnessNames[id] ?? id,
            pending: !!b.revision && b.revision !== c.revision,
          }))
      : [];
  const keyword = search.trim().toLowerCase();
  const rows = all
    .filter(
      (c) =>
        !keyword ||
        [
          c.name,
          c.baseUrl ?? '',
          c.model ?? '',
          ...c.sources.map((s) => s.name),
        ]
          .join(' ')
          .toLowerCase()
          .includes(keyword),
    )
    .map((c) => ({ c, state: connectionState(c, deviceId), used: usage(c) }))
    // Being used by a harness never moves a connection: the order stays put.
    .sort(
      (a, b) =>
        stateOrder[a.state] - stateOrder[b.state] ||
        a.c.name.localeCompare(b.c.name),
    );

  async function startSync(c: ConnectionSummary, source?: ConnectionSource) {
    const sources = syncSources(c, deviceId);
    const chosen = source ?? (sources.length === 1 ? sources[0] : undefined);
    if (!chosen) {
      setChoosing(c);
      return;
    }
    setChoosing(null);
    clearRow(c.id);
    const result = await sync(c, chosen);
    if (result)
      noteRow(
        c.id,
        translate('settings.connSyncedHint', { name: result.name ?? c.name }),
      );
  }

  async function runTest(c: ConnectionSummary) {
    setTesting(c.id);
    clearRow(c.id);
    setRowError(({ [c.id]: _, ...rest }) => rest);
    try {
      const result = await api.test({
        id: c.id,
        ...(c.model ? { model: c.model } : {}),
      });
      if (result.ok === false)
        throw Error(translate('settings.connTestFailed'));
      noteRow(
        c.id,
        typeof result.latencyMs === 'number'
          ? translate('settings.connTestOkNamed', {
              name: c.name,
              ms: result.latencyMs,
            })
          : translate('settings.connTestOkNamedPlain', { name: c.name }),
      );
    } catch (e) {
      setRowError((current) => ({
        ...current,
        [c.id]: connectionErrorMessage(e, translate('settings.connTestFailed')),
      }));
    } finally {
      setTesting(null);
    }
  }

  return (
    <section className="min-w-0" aria-label={translate('settings.connTitle')}>
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-[13px] font-semibold">
          {translate('settings.connTitle')}
        </h3>
        <div className="flex gap-1.5">
          {onRefresh && (
            <button
              type="button"
              className={iconButton}
              aria-label={translate('settings.upstreamsRefresh')}
              title={translate('settings.upstreamsRefresh')}
              disabled={loading}
              onClick={() => {
                setRefreshAsked(true);
                onRefresh();
              }}
            >
              <RefreshCw size={14} aria-hidden />
            </button>
          )}
          {refreshAsked && error && catalog && !loading && (
            <span className="flex items-center">
              <ErrorMark message={error} onRetry={onRefresh} />
            </span>
          )}
          <button
            type="button"
            className={button}
            disabled={!catalog}
            aria-label={translate('settings.connAdd')}
            title={translate('settings.connAdd')}
            onClick={() => setEditing('new')}
          >
            <Plus size={14} aria-hidden />
            {translate('settings.connAddShort')}
          </button>
          {unsupported && (
            <span className="flex items-center">
              <ErrorMark kind="unavailable" message={error} />
            </span>
          )}
        </div>
      </div>
      {notice && (
        <p
          role="status"
          className="mt-2 text-xs text-[var(--theme-accent-strong)]"
        >
          {notice}
        </p>
      )}
      {catalog && deviceId && !catalog.catalogAvailable && (
        // A short neutral line; the reason and Retry are on its mark.
        <p className="mt-3 flex flex-wrap items-center gap-1 text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.connCatalogUnavailable')}
          <ErrorMark
            message={
              catalog.catalogError
                ? translate('settings.connCatalogError', {
                    reason: catalog.catalogError,
                  })
                : translate('settings.connCatalogUnavailable')
            }
            onRetry={() => void reload()}
            retryLabel={translate('settings.connRetry')}
          />
        </p>
      )}
      {all.length > 6 && (
        <input
          type="search"
          className="host-input mt-2.5 min-h-9 w-full rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm"
          aria-label={translate('settings.connSearch')}
          placeholder={translate('settings.connSearch')}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      )}
      {unsupported ? null : error && !catalog ? (
        // Nothing could be read: an obvious Retry, with the reason on its mark.
        <div className="my-3 flex items-center gap-1">
          <button
            type="button"
            className={button}
            onClick={() => void reload()}
          >
            {translate('settings.connRetry')}
          </button>
          <ErrorMark message={error} />
        </div>
      ) : !catalog ? (
        <p role="status" className="py-4 text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.connLoading')}
        </p>
      ) : !all.length ? (
        <p className="my-3 rounded-xl border border-dashed border-[var(--theme-border)] p-3 text-xs leading-5 text-[var(--theme-fg-muted)]">
          {translate('settings.connEmpty')}
        </p>
      ) : (
        <ul className="mt-2.5 space-y-2">
          {!rows.length && (
            <li className="py-2 text-xs text-[var(--theme-fg-muted)]">
              {translate('settings.connNoMatches')}
            </li>
          )}
          {rows.map(({ c, state, used }) => (
            <ConnectionRow
              key={c.id}
              connection={c}
              state={state}
              deviceId={deviceId}
              usage={used}
              syncing={syncing === c.id}
              testing={testing === c.id}
              error={syncErrors[c.id] ?? rowError[c.id] ?? ''}
              onRetry={
                syncErrors[c.id]
                  ? () => void startSync(c)
                  : rowError[c.id]
                    ? () => void runTest(c)
                    : undefined
              }
              notice={rowNotice[c.id] ?? ''}
              onSync={() => void startSync(c)}
              onEdit={() => setEditing(c)}
              onTest={() => void runTest(c)}
              onDelete={() => setDeleting(c)}
            />
          ))}
        </ul>
      )}
      {editing && (
        <ConnectionDialog
          apiRoot={apiRoot}
          connection={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(saved) => {
            const inUse = editing !== 'new' && usage(editing).length > 0;
            upsert(saved);
            setEditing(null);
            noteRow(
              saved.id,
              translate(
                inUse ? 'settings.connSavedInUse' : 'settings.connSaved',
                { name: saved.name },
              ),
            );
            void reload();
          }}
        />
      )}
      {choosing && (
        <FormDialog
          title={translate('settings.connChooseSource')}
          description={translate('settings.connChooseSourceHint')}
          onClose={() => setChoosing(null)}
        >
          <ul className="space-y-2">
            {syncSources(choosing, deviceId).map((s) => (
              <li key={s.deviceId}>
                <button
                  type="button"
                  className={`${button} w-full justify-between`}
                  onClick={() => void startSync(choosing, s)}
                >
                  <span className="truncate">{s.name}</span>
                  <span className="font-mono text-[var(--theme-fg-muted)]">
                    {s.revision.slice(0, 8)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </FormDialog>
      )}
      {deleting && (
        <DeleteDialog
          name={deleting.name}
          onClose={() => setDeleting(null)}
          onDelete={async () => {
            await api.remove(deleting.id);
            setDeleting(null);
            setNotice(
              translate('settings.connRemoved', { name: deleting.name }),
            );
            await reload();
          }}
        />
      )}
    </section>
  );
}

function ConnectionRow({
  connection: c,
  state,
  deviceId,
  usage,
  syncing,
  testing,
  error,
  onRetry,
  notice,
  onSync,
  onEdit,
  onTest,
  onDelete,
}: {
  connection: ConnectionSummary;
  state: ConnectionState;
  deviceId: string | null;
  usage: Usage[];
  syncing: boolean;
  testing: boolean;
  /** Why this row's last sync or test failed; its mark sits after the name. */
  error: string;
  onRetry: (() => void) | undefined;
  notice: string;
  onSync: () => void;
  onEdit: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const remote = remoteSources(c, deviceId);
  const sources = syncSources(c, deviceId);
  const canSync = !!deviceId && (state !== 'local' || sources.length > 0);
  const window = declaredContextWindow(c);
  const meta = [
    servedProtocols(c).map(protocolShort).join(' + '),
    ...(c.local
      ? [
          c.baseUrl?.replace(/^https?:\/\//, ''),
          c.model,
          window &&
            `${translate('settings.contextWindow')} ${tokenCount(window)}`,
        ]
      : [
          translate('settings.connOnDevices', {
            devices: remote.map((s) => s.name).join(', '),
          }),
        ]),
  ]
    .filter(Boolean)
    .join(' · ');
  return (
    <li
      className="rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] px-3 py-2.5"
      aria-label={c.name}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
        <div className="min-w-0 flex-1 basis-36">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <h4
              className="min-w-0 max-w-full truncate text-[13px] font-semibold leading-5"
              title={c.name}
            >
              {c.name}
            </h4>
            {error && <ErrorMark message={error} onRetry={onRetry} />}
            {(state === 'offline' || state === 'conflict') && (
              <span
                className={`inline-flex shrink-0 rounded-full px-1.5 text-[11px] leading-[18px] ${warnPill}`}
              >
                {translate(
                  state === 'offline'
                    ? 'settings.connStateOffline'
                    : 'settings.connStateConflict',
                )}
              </span>
            )}
            {usage.map((u) => (
              <span
                key={u.harness}
                // Small: one connection can serve several harnesses.
                className={`inline-block min-w-0 max-w-[9rem] shrink truncate rounded px-1 text-[10px] leading-[15px] ${u.pending ? warnPill : 'bg-[var(--theme-hover)] text-[var(--theme-fg-soft)]'}`}
                title={translate(
                  u.pending
                    ? 'settings.connPendingHint'
                    : 'settings.connUsedBy',
                  { harness: u.harness },
                )}
              >
                {u.pending
                  ? translate('settings.connUsagePending', {
                      harness: u.harness,
                    })
                  : u.harness}
              </span>
            ))}
          </div>
          <p
            className="mt-0.5 truncate text-[11px] leading-4 text-[var(--theme-fg-muted)]"
            title={meta}
          >
            {meta}
          </p>
        </div>
        <div
          className="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-1.5"
          data-testid="connection-actions"
        >
          {canSync && (
            <button
              type="button"
              className={button}
              disabled={syncing || !sources.length}
              aria-label={translate('settings.connSyncNamed', { name: c.name })}
              title={
                sources.length
                  ? translate('settings.connSyncHint')
                  : translate('settings.connStateOffline')
              }
              onClick={onSync}
            >
              <CloudDownload size={13} aria-hidden />
              {syncing
                ? translate('settings.connSyncing')
                : translate('settings.connSync')}
            </button>
          )}
          {c.local && (
            <RowMenu
              name={c.name}
              testing={testing}
              onEdit={onEdit}
              onTest={onTest}
              onDelete={onDelete}
            />
          )}
        </div>
      </div>
      {!error && notice && (
        <p
          role="status"
          className="mt-1.5 break-words text-xs text-[var(--theme-accent-strong)]"
        >
          {notice}
        </p>
      )}
    </li>
  );
}

function RowMenu({
  name,
  testing,
  onEdit,
  onTest,
  onDelete,
}: {
  name: string;
  testing: boolean;
  onEdit: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const items = () =>
    Array.from(
      list.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)',
      ) ?? [],
    );
  useEffect(() => {
    if (!open) return;
    // Opening focuses the first item; Esc closes and returns focus.
    items()[0]?.focus();
    const outside = (e: PointerEvent) => {
      if (!menu.current?.contains(e.target as Node)) setOpen(false);
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape, true);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape, true);
    };
  }, [open]);
  function move(e: ReactKeyboardEvent) {
    const all = items();
    const index = all.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === 'ArrowDown'
        ? all[(index + 1) % all.length]
        : e.key === 'ArrowUp'
          ? all[(index - 1 + all.length) % all.length]
          : e.key === 'Home'
            ? all[0]
            : e.key === 'End'
              ? all.at(-1)
              : undefined;
    if (!next) return;
    e.preventDefault();
    next.focus();
  }
  function choose(action: () => void) {
    setOpen(false);
    trigger.current?.focus();
    action();
  }
  const item =
    'flex min-h-11 w-full items-center gap-2 rounded-lg px-3 text-left text-xs hover:bg-[var(--theme-hover)] focus-visible:bg-[var(--theme-hover)] disabled:opacity-40 sm:min-h-10';
  return (
    <div className="relative" ref={menu}>
      <button
        ref={trigger}
        type="button"
        className={iconButton}
        aria-label={translate('settings.upstreamsMoreActions', { name })}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <MoreHorizontal size={16} />
      </button>
      {open && (
        <div
          ref={list}
          className="absolute right-0 top-11 z-10 min-w-44 rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-lg sm:top-10"
          role="menu"
          aria-label={translate('settings.upstreamsMoreActions', { name })}
          onKeyDown={move}
        >
          <button
            type="button"
            role="menuitem"
            className={item}
            aria-label={translate('settings.connEditNamed', { name })}
            onClick={() => choose(onEdit)}
          >
            <Pencil size={14} aria-hidden />
            {translate('settings.connEdit')}
          </button>
          <button
            type="button"
            role="menuitem"
            className={item}
            disabled={testing}
            onClick={() => choose(onTest)}
          >
            <FlaskConical size={14} aria-hidden />
            {translate(testing ? 'settings.connTesting' : 'settings.connTest')}
          </button>
          {/* At least 8px between the destructive item and the others. */}
          <div className="my-1 border-t border-[var(--theme-border)]" />
          <button
            type="button"
            role="menuitem"
            className={`${item} text-[var(--status-danger-fg)]`}
            onClick={() => choose(onDelete)}
          >
            <Trash2 size={14} aria-hidden />
            {translate('settings.connRemoveHere')}
          </button>
        </div>
      )}
    </div>
  );
}

function DeleteDialog({
  name,
  onClose,
  onDelete,
}: {
  name: string;
  onClose: () => void;
  onDelete: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return (
    <FormDialog
      title={translate('settings.connRemoveTitle')}
      busy={busy}
      onClose={onClose}
    >
      <p className="text-sm leading-6">
        {translate('settings.connRemoveHint')}
      </p>
      <p className="my-3 text-sm font-medium">{name}</p>
      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          autoFocus
          className="host-secondary-button min-h-10 rounded-lg border px-3 text-[13px] sm:min-h-9"
          disabled={busy}
          onClick={onClose}
        >
          {translate('settings.connCancel')}
        </button>
        <button
          type="button"
          className="min-h-10 rounded-lg border border-[var(--status-danger-border,var(--status-danger-fg))] bg-[var(--status-danger-bg,transparent)] px-3 text-[13px] font-semibold text-[var(--status-danger-fg)] disabled:opacity-40 sm:min-h-9"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            setError('');
            onDelete()
              .catch((e: unknown) =>
                setError(
                  connectionErrorMessage(
                    e,
                    translate('settings.connRemoveFailed'),
                  ),
                ),
              )
              .finally(() => setBusy(false));
          }}
        >
          {busy
            ? translate('settings.connRemoving')
            : translate('settings.connRemoveHere')}
        </button>
        {error && !busy && <ErrorMark message={error} />}
      </div>
    </FormDialog>
  );
}
