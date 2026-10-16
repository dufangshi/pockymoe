import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Check,
  CloudDownload,
  FlaskConical,
  MoreHorizontal,
  Pencil,
  Plus,
  Trash2,
  TriangleAlert,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  connectionErrorMessage,
  connectionState,
  connectionsApi,
  harnessProtocols,
  protocolLabel,
  protocolShort,
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
  'host-secondary-button inline-flex min-h-9 shrink-0 items-center justify-center gap-1 rounded-lg border px-2.5 text-xs font-medium whitespace-nowrap disabled:opacity-40';
const iconButton =
  'host-secondary-button inline-flex min-h-9 min-w-9 shrink-0 items-center justify-center rounded-lg border disabled:opacity-40';
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
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-[13px] font-semibold">
          {translate('settings.connTitle')}
        </h3>
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
      </div>
      {harness && catalog && (
        <p
          className="mt-2.5 flex min-w-0 items-center gap-1.5 rounded-lg bg-[var(--theme-hover)] px-2.5 py-1.5 text-xs"
          data-testid="harness-binding"
        >
          <span className="shrink-0 text-[var(--theme-fg-muted)]">
            {translate('settings.connHarnessUses', { harness: harness.name })}
          </span>
          {/* Pending changes show on the bound row, next to its Reapply. */}
          <strong
            className="min-w-0 truncate font-semibold"
            title={
              bound
                ? [bound.name, binding?.model].filter(Boolean).join(' · ')
                : undefined
            }
          >
            {bound
              ? [bound.name, binding?.model].filter(Boolean).join(' · ')
              : translate(
                  binding?.profileId
                    ? 'settings.connHarnessLegacy'
                    : 'settings.connHarnessNative',
                )}
          </strong>
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
          className="host-input mt-2.5 min-h-9 w-full rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm"
          aria-label={translate('settings.connSearch')}
          placeholder={translate('settings.connSearch')}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      )}
      {error && !catalog ? (
        <div
          role="alert"
          className="my-3 text-xs text-[var(--status-danger-fg)]"
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
  // One status per row; the harness name is already in the page context.
  const status =
    harness && bound
      ? pending
        ? {
            text: translate('settings.connPending'),
            tone: 'warn',
            title: translate('settings.connPendingHint', {
              harness: harness.name,
            }),
          }
        : { text: translate('settings.connInUse'), tone: 'accent' }
      : state === 'offline'
        ? { text: translate('settings.connStateOffline'), tone: 'warn' }
        : state === 'conflict'
          ? { text: translate('settings.connStateConflict'), tone: 'warn' }
          : null;
  const meta = [
    protocolShort(c.protocol),
    ...(c.local
      ? [c.baseUrl?.replace(/^https?:\/\//, ''), c.model]
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
      className={`rounded-xl border px-3 py-2.5 ${bound ? 'border-[var(--theme-accent-border)] bg-[var(--theme-accent-soft)]' : 'border-[var(--theme-border)] bg-[var(--theme-panel)]'}`}
      aria-label={c.name}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
        <div className="min-w-0 flex-1 basis-36">
          <div className="flex min-w-0 items-center gap-1.5">
            <h4
              className="min-w-0 truncate text-[13px] font-semibold leading-5"
              title={c.name}
            >
              {c.name}
            </h4>
            {status && (
              <span
                className={`inline-flex shrink-0 items-center gap-0.5 rounded-full px-1.5 text-[11px] leading-[18px] ${status.tone === 'accent' ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-strong)]' : 'bg-[var(--status-warning-bg,var(--theme-hover))] text-[var(--status-warning-fg,var(--theme-fg))]'}`}
                title={status.title}
              >
                {status.tone === 'accent' && <Check size={11} aria-hidden />}
                {status.text}
              </span>
            )}
            {/* A bound row's pill shows the harness state; keep the device split visible. */}
            {status && state === 'conflict' && harness && bound && (
              <span
                role="img"
                className="inline-flex shrink-0 text-[var(--status-warning-fg,var(--theme-fg))]"
                aria-label={translate('settings.connStateConflict')}
                title={translate('settings.connStateConflict')}
              >
                <TriangleAlert size={13} aria-hidden />
              </span>
            )}
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
          {harness &&
            c.local &&
            (compatible ? (
              <button
                type="button"
                className={
                  pending
                    ? `${button} border-[var(--theme-accent-border)] text-[var(--theme-accent-strong)]`
                    : button
                }
                disabled={!harness.installed}
                aria-label={translate(
                  bound
                    ? 'settings.connReapplyNamed'
                    : 'settings.connApplyNamed',
                  { name: c.name, harness: harness.name },
                )}
                title={
                  harness.installed
                    ? translate(
                        bound
                          ? 'settings.connReapplyNamed'
                          : 'settings.connApplyNamed',
                        { name: c.name, harness: harness.name },
                      )
                    : translate('settings.upstreamsUninstalled')
                }
                onClick={onApply}
              >
                {translate(
                  bound
                    ? 'settings.connReapplyShort'
                    : 'settings.connApplyShort',
                )}
              </button>
            ) : (
              <span
                className="px-1 text-[11px] text-[var(--theme-fg-muted)]"
                title={translate('settings.connIncompatibleHint', {
                  harness: harness.name,
                  protocol: protocolLabel(c.protocol),
                })}
              >
                {translate('settings.connIncompatible')}
              </span>
            ))}
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
      {error && (
        <p
          role="alert"
          className="mt-1.5 break-words text-xs text-[var(--status-danger-fg)]"
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
          className="absolute right-0 top-10 z-10 min-w-44 rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-lg"
          role="group"
          aria-label={translate('settings.upstreamsMoreActions', { name })}
        >
          <button
            type="button"
            className={item}
            aria-label={translate('settings.connEditNamed', { name })}
            onClick={() => {
              setOpen(false);
              trigger.current?.focus();
              onEdit();
            }}
          >
            <Pencil size={14} aria-hidden />
            {translate('settings.connEdit')}
          </button>
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
            className="host-secondary-button min-h-9 rounded-lg border px-3 text-[13px]"
            disabled={busy}
            onClick={onClose}
          >
            {translate('settings.connCancel')}
          </button>
          <button
            type="submit"
            className="relay-button-primary !min-h-9"
            disabled={busy || !model.trim()}
          >
            {busy
              ? translate('settings.connApplying')
              : translate('settings.connApplyShort')}
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
          className="host-secondary-button min-h-9 rounded-lg border px-3 text-[13px]"
          disabled={busy}
          onClick={onClose}
        >
          {translate('settings.connCancel')}
        </button>
        <button
          type="button"
          className="relay-button-primary !min-h-9"
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
