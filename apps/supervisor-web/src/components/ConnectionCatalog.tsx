import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Check,
  CloudDownload,
  FlaskConical,
  MoreHorizontal,
  Pencil,
  Play,
  Plus,
  Trash2,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  connectionErrorMessage,
  connectionState,
  connectionsApi,
  harnessProtocols,
  protocolLabel,
  remoteSources,
  syncSources,
  type ConnectionSource,
  type ConnectionState,
  type ConnectionSummary,
} from '../lib/connections';
import { useConnectionCatalog } from '../lib/useConnectionCatalog';
import { ConnectionDialog } from './ConnectionDialog';
import { ConnectionModelField } from './ConnectionModelField';
import { FormDialog } from './FormDialog';

const button =
  'host-secondary-button inline-flex min-h-10 shrink-0 items-center justify-center gap-1.5 rounded-lg border px-3 text-xs disabled:opacity-40';
// Long harness names may shrink and truncate instead of pushing actions off-screen.
const flexButton = `${button.replace(' shrink-0', '')} min-w-0 max-w-full`;
const iconButton =
  'host-secondary-button inline-flex min-h-10 min-w-10 shrink-0 items-center justify-center rounded-lg border disabled:opacity-40';
const stateOrder: Record<ConnectionState, number> = {
  local: 0,
  conflict: 1,
  remote: 2,
  offline: 3,
};

export type HarnessContext = { id: string; name: string; installed: boolean };

/**
 * The account's connections as seen from one device. Sync copies a remote connection
 * here; Apply (harness context only) is a separate, explicit action on local entries.
 */
export function ConnectionCatalog({
  apiRoot,
  harness,
  reloadKey = 0,
}: {
  apiRoot: string;
  harness?: HarnessContext | null;
  /** Bumped by the page's Refresh and by native-config tools that migrate profiles. */
  reloadKey?: number;
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
  } = useConnectionCatalog(apiRoot);
  const [editing, setEditing] = useState<ConnectionSummary | 'new' | null>(
    null,
  );
  const [applying, setApplying] = useState<ConnectionSummary | null>(null);
  const [choosing, setChoosing] = useState<ConnectionSummary | null>(null);
  const [deleting, setDeleting] = useState<ConnectionSummary | null>(null);
  const [notice, setNotice] = useState('');
  const [rowError, setRowError] = useState<Record<string, string>>({});
  const [testing, setTesting] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const firstReload = useRef(reloadKey);
  useEffect(() => {
    if (reloadKey !== firstReload.current) void reload();
  }, [reloadKey]);
  const deviceId = catalog?.deviceId ?? null;
  const binding = harness ? catalog?.bindings[harness.id] : undefined;
  const all = catalog?.connections ?? [];
  const bound = binding?.connectionId
    ? all.find((c) => c.id === binding.connectionId && c.local)
    : undefined;
  // Saving or syncing never changes the harness: the applied revision stays until Reapply.
  const pending =
    !!bound && !!binding?.revision && binding.revision !== bound.revision;
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
    .map((c) => ({ c, state: connectionState(c, deviceId) }))
    .sort(
      (a, b) =>
        Number(b.c.id === bound?.id) - Number(a.c.id === bound?.id) ||
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
    setNotice('');
    const result = await sync(c, chosen);
    if (result)
      setNotice(
        translate(
          harness ? 'settings.connSyncedApplyHint' : 'settings.connSyncedHint',
          { name: result.name ?? c.name, harness: harness?.name ?? '' },
        ),
      );
  }

  async function runTest(c: ConnectionSummary) {
    setTesting(c.id);
    setNotice('');
    setRowError(({ [c.id]: _, ...rest }) => rest);
    try {
      const result = await api.test({
        id: c.id,
        ...(c.model ? { model: c.model } : {}),
      });
      if (result.ok === false)
        throw Error(translate('settings.connTestFailed'));
      setNotice(
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
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">
          {translate('settings.connTitle')}
        </h3>
        <div className="flex gap-2">
          <button
            type="button"
            className={button}
            disabled={!catalog}
            onClick={() => setEditing('new')}
          >
            <Plus size={14} />
            {translate('settings.connAdd')}
          </button>
        </div>
      </div>
      {harness && catalog && (
        <p
          className="mt-3 flex min-w-0 flex-wrap items-center gap-x-1.5 rounded-lg bg-[var(--theme-hover)] px-3 py-2 text-xs"
          data-testid="harness-binding"
        >
          <span className="text-[var(--theme-fg-muted)]">
            {translate('settings.connHarnessUses', { harness: harness.name })}
          </span>
          {bound ? (
            <>
              <strong className="min-w-0 truncate font-semibold">
                {bound.name}
              </strong>
              {binding?.model && (
                <span className="min-w-0 truncate text-[var(--theme-fg-muted)]">
                  · {binding.model}
                </span>
              )}
              {pending && (
                <span className="text-[var(--status-warning-fg,var(--status-danger-fg))]">
                  · {translate('settings.connPending')}
                </span>
              )}
            </>
          ) : (
            <strong className="font-semibold">
              {translate(
                binding?.profileId
                  ? 'settings.connHarnessLegacy'
                  : 'settings.connHarnessNative',
              )}
            </strong>
          )}
        </p>
      )}
      {catalog && deviceId && !catalog.catalogAvailable && (
        <p
          role="status"
          className="mt-3 flex flex-wrap items-center gap-2 text-xs text-[var(--theme-fg-muted)]"
        >
          {catalog.catalogError
            ? translate('settings.connCatalogError', {
                reason: catalog.catalogError,
              })
            : translate('settings.connCatalogUnavailable')}
          <button
            type="button"
            className="underline underline-offset-2"
            onClick={() => void reload()}
          >
            {translate('settings.connRetry')}
          </button>
        </p>
      )}
      {all.length > 6 && (
        <input
          type="search"
          className="host-input mt-3 w-full rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2 text-sm"
          aria-label={translate('settings.connSearch')}
          placeholder={translate('settings.connSearch')}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      )}
      {error && !catalog ? (
        <div
          role="alert"
          className="my-4 text-sm text-[var(--status-danger-fg)]"
        >
          <p>{error}</p>
          <button
            type="button"
            className={`${button} mt-2 text-[var(--theme-fg)]`}
            onClick={() => void reload()}
          >
            {translate('settings.connRetry')}
          </button>
        </div>
      ) : !catalog ? (
        <p role="status" className="py-5 text-sm text-[var(--theme-fg-muted)]">
          {translate('settings.connLoading')}
        </p>
      ) : !all.length ? (
        <p className="my-4 rounded-xl border border-dashed border-[var(--theme-border)] p-4 text-sm text-[var(--theme-fg-muted)]">
          {translate('settings.connEmpty')}
        </p>
      ) : (
        <ul className="mt-3 space-y-2">
          {!rows.length && (
            <li className="py-3 text-sm text-[var(--theme-fg-muted)]">
              {translate('settings.connNoMatches')}
            </li>
          )}
          {rows.map(({ c, state }) => (
            <ConnectionRow
              key={c.id}
              connection={c}
              state={state}
              deviceId={deviceId}
              harness={harness ?? null}
              protocols={
                harness
                  ? (catalog.harnessProtocols?.[harness.id] ??
                    harnessProtocols[harness.id] ??
                    [])
                  : []
              }
              bound={bound?.id === c.id}
              pending={pending && bound?.id === c.id}
              syncing={syncing === c.id}
              testing={testing === c.id}
              error={syncErrors[c.id] ?? rowError[c.id] ?? ''}
              onSync={() => void startSync(c)}
              onApply={() => setApplying(c)}
              onEdit={() => setEditing(c)}
              onTest={() => void runTest(c)}
              onDelete={() => setDeleting(c)}
            />
          ))}
        </ul>
      )}
      {error && catalog && (
        <p role="alert" className="mt-3 text-xs text-[var(--status-danger-fg)]">
          {error}
        </p>
      )}
      {notice && (
        <p
          role="status"
          className="mt-3 text-xs text-[var(--theme-accent-strong)]"
        >
          {notice}
        </p>
      )}
      {editing && (
        <ConnectionDialog
          apiRoot={apiRoot}
          connection={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(saved) => {
            upsert(saved);
            setEditing(null);
            setNotice(
              harness && saved.id === bound?.id
                ? translate('settings.connSavedBound', {
                    name: saved.name,
                    harness: harness.name,
                  })
                : translate('settings.connSaved', { name: saved.name }),
            );
            void reload();
          }}
        />
      )}
      {applying && harness && (
        <ApplyDialog
          apiRoot={apiRoot}
          connection={applying}
          initialModel={
            (applying.id === bound?.id ? binding?.model : null) ||
            applying.model ||
            ''
          }
          harness={harness}
          onClose={() => setApplying(null)}
          onApplied={(model) => {
            setApplying(null);
            setNotice(
              translate('settings.connApplied', {
                name: applying.name,
                harness: harness.name,
                model,
              }),
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
  harness,
  protocols,
  bound,
  pending,
  syncing,
  testing,
  error,
  onSync,
  onApply,
  onEdit,
  onTest,
  onDelete,
}: {
  connection: ConnectionSummary;
  state: ConnectionState;
  deviceId: string | null;
  harness: HarnessContext | null;
  protocols: string[];
  bound: boolean;
  pending: boolean;
  syncing: boolean;
  testing: boolean;
  error: string;
  onSync: () => void;
  onApply: () => void;
  onEdit: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const remote = remoteSources(c, deviceId);
  const sources = syncSources(c, deviceId);
  const canSync = !!deviceId && (state !== 'local' || sources.length > 0);
  const compatible = !harness || protocols.includes(c.protocol);
  const chip = {
    local: null,
    remote: translate('settings.connStateRemote'),
    offline: translate('settings.connStateOffline'),
    conflict: translate('settings.connStateConflict'),
  }[state];
  return (
    <li
      className={`rounded-xl border p-3 ${bound ? 'border-[var(--theme-accent-border)] bg-[var(--theme-accent-soft)]' : 'border-[var(--theme-border)] bg-[var(--theme-panel)]'}`}
      aria-label={c.name}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div className="min-w-0 flex-1 basis-40">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <h4
              className="min-w-0 truncate text-sm font-semibold"
              title={c.name}
            >
              {c.name}
            </h4>
            <span className="rounded-md border border-[var(--theme-border)] px-1.5 py-0.5 text-[11px] text-[var(--theme-fg-muted)]">
              {protocolLabel(c.protocol)}
            </span>
            {chip && (
              <span
                className={`rounded-md px-1.5 py-0.5 text-[11px] ${state === 'conflict' || state === 'offline' ? 'bg-[var(--status-warning-bg,var(--theme-hover))] text-[var(--status-warning-fg,var(--theme-fg))]' : 'bg-[var(--theme-hover)] text-[var(--theme-fg-muted)]'}`}
              >
                {chip}
              </span>
            )}
          </div>
          <p className="mt-1 truncate text-xs text-[var(--theme-fg-muted)]">
            {c.local
              ? [c.baseUrl, c.model].filter(Boolean).join(' · ')
              : translate('settings.connOnDevices', {
                  devices: remote.map((s) => s.name).join(', '),
                })}
          </p>
        </div>
        <div
          className="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2"
          data-testid="connection-actions"
        >
          {canSync && (
            <button
              type="button"
              className={button}
              disabled={syncing || !sources.length}
              title={
                sources.length
                  ? translate('settings.connSyncHint')
                  : translate('settings.connStateOffline')
              }
              onClick={onSync}
            >
              <CloudDownload size={14} />
              {syncing
                ? translate('settings.connSyncing')
                : translate('settings.connSync')}
            </button>
          )}
          {harness && c.local && bound && (
            <span
              className={`inline-flex min-h-10 items-center gap-1 px-1 text-xs ${pending ? 'text-[var(--status-warning-fg,var(--status-danger-fg))]' : 'text-[var(--theme-accent-strong)]'}`}
              title={
                pending
                  ? translate('settings.connPendingHint', {
                      harness: harness.name,
                    })
                  : undefined
              }
            >
              {!pending && <Check size={14} />}
              {translate(
                pending ? 'settings.connPending' : 'settings.connInUse',
              )}
            </span>
          )}
          {harness &&
            c.local &&
            (compatible ? (
              <button
                type="button"
                className={
                  pending
                    ? `${flexButton} border-[var(--theme-accent-border)] text-[var(--theme-accent-strong)]`
                    : flexButton
                }
                disabled={!harness.installed}
                title={
                  harness.installed
                    ? undefined
                    : translate('settings.upstreamsUninstalled')
                }
                onClick={onApply}
              >
                <Play size={14} className="shrink-0" />
                <span className="truncate">
                  {translate(
                    bound ? 'settings.connReapply' : 'settings.connApply',
                    { harness: harness.name },
                  )}
                </span>
              </button>
            ) : (
              <span
                className="px-2 text-xs text-[var(--theme-fg-muted)]"
                title={translate('settings.connIncompatibleHint', {
                  harness: harness.name,
                  protocol: protocolLabel(c.protocol),
                })}
              >
                {translate('settings.connIncompatible')}
              </span>
            ))}
          {c.local && (
            <>
              <button
                type="button"
                className={iconButton}
                aria-label={translate('settings.connEditNamed', {
                  name: c.name,
                })}
                title={translate('settings.connEdit')}
                onClick={onEdit}
              >
                <Pencil size={14} />
              </button>
              <RowMenu
                name={c.name}
                testing={testing}
                onTest={onTest}
                onDelete={onDelete}
              />
            </>
          )}
        </div>
      </div>
      {error && (
        <p
          role="alert"
          className="mt-2 break-words text-xs text-[var(--status-danger-fg)]"
        >
          {error}
        </p>
      )}
    </li>
  );
}

function RowMenu({
  name,
  testing,
  onTest,
  onDelete,
}: {
  name: string;
  testing: boolean;
  onTest: () => void;
  onDelete: () => void;
}) {
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
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
  const item =
    'flex min-h-10 w-full items-center gap-2 rounded-lg px-3 text-left text-xs hover:bg-[var(--theme-hover)] disabled:opacity-40';
  return (
    <div className="relative" ref={menu}>
      <button
        ref={trigger}
        type="button"
        className={iconButton}
        aria-label={translate('settings.upstreamsMoreActions', { name })}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <MoreHorizontal size={16} />
      </button>
      {open && (
        <div
          className="absolute right-0 top-12 z-10 min-w-52 rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-lg"
          role="group"
          aria-label={translate('settings.upstreamsMoreActions', { name })}
        >
          <button
            type="button"
            className={item}
            disabled={testing}
            onClick={() => {
              setOpen(false);
              trigger.current?.focus();
              onTest();
            }}
          >
            <FlaskConical size={14} />
            {translate(testing ? 'settings.connTesting' : 'settings.connTest')}
          </button>
          <button
            type="button"
            className={`${item} text-[var(--status-danger-fg)]`}
            onClick={() => {
              setOpen(false);
              trigger.current?.focus();
              onDelete();
            }}
          >
            <Trash2 size={14} />
            {translate('settings.connRemoveHere')}
          </button>
        </div>
      )}
    </div>
  );
}

function ApplyDialog({
  apiRoot,
  connection,
  initialModel,
  harness,
  onClose,
  onApplied,
}: {
  apiRoot: string;
  connection: ConnectionSummary;
  initialModel: string;
  harness: HarnessContext;
  onClose: () => void;
  onApplied: (model: string) => void;
}) {
  const api = useMemo(() => connectionsApi(apiRoot), [apiRoot]);
  const [model, setModel] = useState(initialModel);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return (
    <FormDialog
      title={translate('settings.connApplyTitle', {
        name: connection.name,
        harness: harness.name,
      })}
      busy={busy}
      onClose={onClose}
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (busy || !model.trim()) return;
          setBusy(true);
          setError('');
          api
            .apply(connection.id, harness.id, model.trim())
            .then(() => onApplied(model.trim()))
            .catch((err: unknown) =>
              setError(
                connectionErrorMessage(
                  err,
                  translate('settings.connApplyFailed'),
                ),
              ),
            )
            .finally(() => setBusy(false));
        }}
      >
        <ConnectionModelField
          value={model}
          onChange={setModel}
          canDiscover
          autoDiscover
          discoverKey={connection.id + connection.revision}
          discover={(signal) => api.models({ id: connection.id }, signal)}
          disabled={busy}
        />
        <p className="text-xs leading-5 text-[var(--theme-fg-muted)]">
          {translate('settings.connApplyHint')}
        </p>
        {error && (
          <p
            role="alert"
            className="break-words text-xs text-[var(--status-danger-fg)]"
          >
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            className="host-secondary-button min-h-10 rounded-lg border px-3 text-sm"
            disabled={busy}
            onClick={onClose}
          >
            {translate('settings.connCancel')}
          </button>
          <button
            type="submit"
            className="relay-button-primary min-h-10"
            disabled={busy || !model.trim()}
          >
            {busy
              ? translate('settings.connApplying')
              : translate('settings.connApply', { harness: harness.name })}
          </button>
        </div>
      </form>
    </FormDialog>
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
      {error && (
        <p
          role="alert"
          className="mb-3 break-words text-xs text-[var(--status-danger-fg)]"
        >
          {error}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <button
          type="button"
          className="host-secondary-button min-h-10 rounded-lg border px-3 text-sm"
          disabled={busy}
          onClick={onClose}
        >
          {translate('settings.connCancel')}
        </button>
        <button
          type="button"
          className="relay-button-primary min-h-10"
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
      </div>
    </FormDialog>
  );
}
