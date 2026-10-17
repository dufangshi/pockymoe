import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Settings2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import {
  connectionErrorMessage,
  connectionsApi,
  harnessProtocols,
  type ConnectionSummary,
} from '../lib/connections';
import type { ConnectionCatalogStore } from '../lib/useConnectionCatalog';
import { button, iconButton } from './ConnectionCatalog';
import { ConnectionModelField } from './ConnectionModelField';
import { FormDialog } from './FormDialog';
import { UpstreamManagement } from './UpstreamManagement';

export const managedHarnesses = [
  'codex',
  'claude',
  'gemini',
  'grok',
  'deepseek',
];
/** Select value for a harness's own sign-in or native configuration. */
const NATIVE_CHOICE = ':native';

export type InventoryHarness = {
  id: string;
  name: string;
  base: { installed?: boolean; path?: string } | null;
};

// Base installation is authoritative: an installed ACP adapter alone is not an
// installed harness. Older inventories omit `installed` on success.
export function isInstalled(h: InventoryHarness) {
  return (
    !!h.base &&
    h.base.installed !== false &&
    (h.base.installed === true || !!h.base.path)
  );
}

/**
 * Where saved connections are used. Each managed harness picks one compatible local
 * connection and applies it explicitly; nothing here creates or edits connections.
 */
export function HarnessConnections({
  apiRoot,
  store,
  harnesses,
  loading,
  error,
  onRetry,
}: {
  apiRoot: string;
  store: ConnectionCatalogStore;
  harnesses: InventoryHarness[];
  loading: boolean;
  error: string;
  onRetry: () => void;
}) {
  useI18n();
  const [showUninstalled, setShowUninstalled] = useState(false);
  const [notice, setNotice] = useState('');
  const managed = harnesses.filter((h) => managedHarnesses.includes(h.id));
  const visible = managed.filter((h) => showUninstalled || isInstalled(h));
  return (
    <section
      className="min-w-0 border-t border-[var(--theme-border)] pt-4"
      aria-label={translate('settings.connHarnessesTitle')}
    >
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <h3 className="text-[13px] font-semibold">
          {translate('settings.connHarnessesTitle')}
        </h3>
        {managed.some((h) => !isInstalled(h)) && (
          <label className="flex w-fit items-center gap-2 text-xs text-[var(--theme-fg-muted)]">
            <input
              type="checkbox"
              checked={showUninstalled}
              onChange={(e) => setShowUninstalled(e.target.checked)}
            />
            {translate('settings.upstreamsShowUninstalled')}
          </label>
        )}
      </div>
      <p className="mt-1 text-[11px] leading-4 text-[var(--theme-fg-muted)]">
        {translate('settings.connHarnessesHint')}
      </p>
      {loading ? (
        <p role="status" className="py-3 text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.loading')}
        </p>
      ) : error ? (
        <div
          role="alert"
          className="mt-2 text-xs text-[var(--status-danger-fg)]"
        >
          <p>{error}</p>
          <button
            type="button"
            className={`${button} mt-2 text-[var(--theme-fg)]`}
            onClick={onRetry}
          >
            {translate('settings.connRetry')}
          </button>
        </div>
      ) : !visible.length ? (
        <p className="mt-2 rounded-xl border border-dashed border-[var(--theme-border)] p-3 text-xs leading-5 text-[var(--theme-fg-muted)]">
          {translate('settings.connHarnessesNone')}
        </p>
      ) : (
        <ul className="mt-2.5 space-y-2">
          {visible.map((h) => (
            <HarnessRow
              key={h.id}
              apiRoot={apiRoot}
              store={store}
              harness={h}
              onApplied={setNotice}
            />
          ))}
        </ul>
      )}
      {notice && (
        <p
          role="status"
          className="mt-3 text-xs text-[var(--theme-accent-strong)]"
        >
          {notice}
        </p>
      )}
    </section>
  );
}

function HarnessRow({
  apiRoot,
  store,
  harness,
  onApplied,
}: {
  apiRoot: string;
  store: ConnectionCatalogStore;
  harness: InventoryHarness;
  onApplied: (notice: string) => void;
}) {
  const { catalog, reload } = store;
  const installed = isInstalled(harness);
  const binding = catalog?.bindings[harness.id];
  const local = (catalog?.connections ?? []).filter((c) => c.local);
  const bound = binding?.connectionId
    ? local.find((c) => c.id === binding.connectionId)
    : undefined;
  const protocols =
    catalog?.harnessProtocols?.[harness.id] ??
    harnessProtocols[harness.id] ??
    [];
  const compatible = local.filter((c) => protocols.includes(c.protocol));
  // The harness's own sign-in or native configuration: offered here only, and only
  // when the device reports it (older Supervisors report nothing).
  const native = catalog?.native?.[harness.id];
  const nativeOption = native?.available ? native : undefined;
  const nativeLabel = native
    ? translate(
        native.kind === 'oauth'
          ? 'settings.connNativeOAuth'
          : 'settings.connNativeConfig',
      )
    : '';
  const nativeActive = !!native?.active && !binding?.connectionId;
  // Follows what is applied until the user picks something else.
  const [picked, setPicked] = useState<string | null>(null);
  const choice =
    picked ?? bound?.id ?? (nativeActive && nativeOption ? NATIVE_CHOICE : '');
  const nativeChosen = choice === NATIVE_CHOICE && !!nativeOption;
  const chosen = nativeChosen
    ? undefined
    : compatible.find((c) => c.id === choice);
  const reapply = !!chosen && chosen.id === bound?.id;
  const [applyingNative, setApplyingNative] = useState(false);
  // Saving or syncing never changes a harness: it keeps the applied revision.
  const pending =
    !!bound && !!binding?.revision && binding.revision !== bound.revision;
  const [applying, setApplying] = useState<ConnectionSummary | null>(null);
  const [nativeOpen, setNativeOpen] = useState(false);
  const current = bound
    ? [bound.name, binding?.model].filter(Boolean).join(' · ')
    : nativeActive
      ? nativeLabel
      : translate(
          binding?.profileId
            ? 'settings.connHarnessLegacy'
            : 'settings.connHarnessNative',
        );
  return (
    <li
      className="rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] px-3 py-2.5"
      aria-label={harness.name}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div className="min-w-0 flex-1 basis-40">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <h4 className="min-w-0 truncate text-[13px] font-semibold leading-5">
              {harness.name}
            </h4>
            {!installed && (
              <span className="inline-flex shrink-0 rounded-full bg-[var(--theme-hover)] px-1.5 text-[11px] leading-[18px] text-[var(--theme-fg-muted)]">
                {translate('settings.connNotInstalled')}
              </span>
            )}
            {pending && (
              <span
                className="inline-flex shrink-0 rounded-full bg-[var(--status-warning-bg,var(--theme-hover))] px-1.5 text-[11px] leading-[18px] text-[var(--status-warning-fg,var(--theme-fg))]"
                title={translate('settings.connPendingHint', {
                  harness: harness.name,
                })}
              >
                {translate('settings.connPending')}
              </span>
            )}
          </div>
          <p
            className="mt-0.5 truncate text-[11px] leading-4 text-[var(--theme-fg-muted)]"
            title={current}
            data-testid="harness-current"
          >
            {current}
          </p>
        </div>
        <div className="flex min-w-0 flex-1 basis-60 items-center gap-1.5 sm:ml-auto sm:max-w-md">
          <select
            className="host-input min-h-10 min-w-0 flex-1 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2 py-1.5 text-xs sm:min-h-9"
            aria-label={translate('settings.connHarnessChoose', {
              harness: harness.name,
            })}
            value={nativeChosen ? NATIVE_CHOICE : chosen ? choice : ''}
            disabled={!catalog || (!compatible.length && !nativeOption)}
            onChange={(e) => setPicked(e.target.value)}
          >
            <option value="">
              {translate(
                compatible.length || nativeOption
                  ? 'settings.connHarnessPick'
                  : 'settings.connHarnessNoneCompatible',
              )}
            </option>
            {nativeOption && (
              <option value={NATIVE_CHOICE}>{nativeLabel}</option>
            )}
            {compatible.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            className={
              pending && reapply
                ? `${button} border-[var(--theme-accent-border)] text-[var(--theme-accent-strong)]`
                : button
            }
            // Choosing the native option already in use changes nothing.
            disabled={
              !installed ||
              (!chosen && !nativeChosen) ||
              (nativeChosen && nativeActive)
            }
            aria-label={
              nativeChosen
                ? translate('settings.connApplyNamed', {
                    name: nativeLabel,
                    harness: harness.name,
                  })
                : chosen
                  ? translate(
                      reapply
                        ? 'settings.connReapplyNamed'
                        : 'settings.connApplyNamed',
                      { name: chosen.name, harness: harness.name },
                    )
                  : undefined
            }
            title={
              installed ? undefined : translate('settings.upstreamsUninstalled')
            }
            onClick={() =>
              nativeChosen
                ? setApplyingNative(true)
                : chosen && setApplying(chosen)
            }
          >
            {translate(
              reapply ? 'settings.connReapplyShort' : 'settings.connApplyShort',
            )}
          </button>
          <button
            type="button"
            className={iconButton}
            aria-expanded={nativeOpen}
            aria-label={translate('settings.connAdvancedNative', {
              harness: harness.name,
            })}
            title={translate('settings.connAdvancedNative', {
              harness: harness.name,
            })}
            onClick={() => setNativeOpen(!nativeOpen)}
          >
            <Settings2 size={14} aria-hidden />
          </button>
        </div>
      </div>
      {nativeOpen && (
        <UpstreamManagement
          key={apiRoot + harness.id}
          apiRoot={apiRoot}
          harness={harness.id}
          appearance="advanced"
          harnessName={harness.name}
          installed={installed}
          onChanged={() => void reload()}
        />
      )}
      {applyingNative && nativeOption && (
        <NativeApplyDialog
          apiRoot={apiRoot}
          harness={harness}
          label={nativeLabel}
          {...(nativeOption.revision
            ? { revision: nativeOption.revision }
            : {})}
          onClose={() => setApplyingNative(false)}
          onApplied={() => {
            setApplyingNative(false);
            setPicked(null);
            onApplied(
              translate('settings.connNativeApplied', {
                name: nativeLabel,
                harness: harness.name,
              }),
            );
            void reload();
          }}
        />
      )}
      {applying && (
        <ApplyDialog
          apiRoot={apiRoot}
          connection={applying}
          initialModel={
            (reapply ? binding?.model : null) || applying.model || ''
          }
          harness={harness}
          onClose={() => setApplying(null)}
          onApplied={(model) => {
            const name = applying.name;
            setApplying(null);
            onApplied(
              translate('settings.connApplied', {
                name,
                harness: harness.name,
                model,
              }),
            );
            void reload();
          }}
        />
      )}
    </li>
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
  harness: { id: string; name: string };
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
            className="host-secondary-button min-h-10 rounded-lg border px-3 text-[13px] sm:min-h-9"
            disabled={busy}
            onClick={onClose}
          >
            {translate('settings.connCancel')}
          </button>
          <button
            type="submit"
            className="relay-button-primary !min-h-10 sm:!min-h-9"
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

/**
 * Confirms switching a harness back to its own sign-in or native configuration.
 * No model or connection fields: the harness keeps its own.
 */
function NativeApplyDialog({
  apiRoot,
  harness,
  label,
  revision,
  onClose,
  onApplied,
}: {
  apiRoot: string;
  harness: { id: string; name: string };
  label: string;
  revision?: string;
  onClose: () => void;
  onApplied: () => void;
}) {
  const api = useMemo(() => connectionsApi(apiRoot), [apiRoot]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return (
    <FormDialog
      title={translate('settings.connApplyTitle', {
        name: label,
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
          if (busy) return;
          setBusy(true);
          setError('');
          api
            .activateNative(harness.id, revision)
            .then(onApplied)
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
            className="host-secondary-button min-h-10 rounded-lg border px-3 text-[13px] sm:min-h-9"
            disabled={busy}
            onClick={onClose}
          >
            {translate('settings.connCancel')}
          </button>
          <button
            type="submit"
            className="relay-button-primary !min-h-10 sm:!min-h-9"
            disabled={busy}
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
