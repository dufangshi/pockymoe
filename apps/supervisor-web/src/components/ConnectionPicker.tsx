import {
  ErrorMark,
  type UpstreamConnectionPickerProps,
} from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { CloudDownload, Pencil, Plus } from 'lucide-react';
import { useState } from 'react';
import {
  connectionProtocols,
  connectionState,
  protocolLabel,
  remoteSources,
  syncSources,
  type ConnectionSource,
  type ConnectionSummary,
} from '../lib/connections';
import { useConnectionCatalog } from '../lib/useConnectionCatalog';
import { ConnectionDialog } from './ConnectionDialog';
import { ConnectionModelField } from './ConnectionModelField';

const field =
  'host-input min-h-10 min-w-0 flex-1 sm:min-h-9 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm';
const iconButton =
  'host-secondary-button inline-flex min-h-10 min-w-10 shrink-0 sm:min-h-9 sm:min-w-9 items-center justify-center gap-1 rounded-lg border px-2 text-xs whitespace-nowrap disabled:opacity-40';

/**
 * Chooses a saved connection for a plugin. Choosing only changes the caller's unsaved
 * form; remote connections must be synced to this device before they can be chosen.
 */
export function ConnectionPicker({
  apiRoot,
  connectionId,
  model,
  protocols = connectionProtocols,
  disabled = false,
  onChange,
}: UpstreamConnectionPickerProps & { apiRoot: string }) {
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
  const [notice, setNotice] = useState('');
  const [choosing, setChoosing] = useState<string | null>(null);
  const deviceId = catalog?.deviceId ?? null;
  const all = catalog?.connections ?? [];
  const local = all.filter((c) => c.local);
  const selected = local.find((c) => c.id === connectionId);
  const remote = all.filter(
    (c) =>
      !c.local &&
      protocols.includes(c.protocol) &&
      remoteSources(c, deviceId).length > 0,
  );
  const missing = !!connectionId && !!catalog && !selected;
  // Only a catalog that never arrived gets a mark; a later reload keeps it quietly.
  const failed = !!error && !catalog;
  const unsupported =
    failed && error === translate('settings.connUpdateSupervisor');

  async function syncFrom(c: ConnectionSummary, source: ConnectionSource) {
    setChoosing(null);
    const result = await sync(c, source);
    if (result)
      setNotice(
        translate('settings.connSyncedChoose', { name: result.name ?? c.name }),
      );
  }

  function choose(id: string) {
    const next = local.find((c) => c.id === id);
    setNotice('');
    onChange({
      connectionId: id,
      model: next?.model || (id ? model : ''),
    });
  }

  return (
    <div className="min-w-0 space-y-2">
      <div className="flex min-w-0 gap-2">
        <select
          className={field}
          aria-label={translate('settings.connPickerLabel')}
          value={connectionId}
          disabled={disabled || !catalog}
          onChange={(e) => choose(e.target.value)}
        >
          <option value="">
            {loading && !catalog
              ? translate('settings.connLoading')
              : translate('settings.connPickerNone')}
          </option>
          {missing && (
            <option value={connectionId}>
              {translate('settings.connPickerMissing')}
            </option>
          )}
          {local.map((c) => (
            <option
              key={c.id}
              value={c.id}
              disabled={!protocols.includes(c.protocol)}
            >
              {protocols.includes(c.protocol)
                ? c.name
                : translate('settings.connPickerOtherProtocol', {
                    name: c.name,
                    protocol: protocolLabel(c.protocol),
                  })}
            </option>
          ))}
        </select>
        {missing && (
          <span className="flex shrink-0 items-center">
            <ErrorMark
              kind="unavailable"
              message={translate('settings.connPickerMissingHint')}
            />
          </span>
        )}
        <button
          type="button"
          className={iconButton}
          disabled={disabled || !selected}
          aria-label={translate('settings.connEditNamed', {
            name: selected?.name ?? '',
          })}
          title={translate('settings.connEdit')}
          onClick={() => selected && setEditing(selected)}
        >
          <Pencil size={14} />
        </button>
        <button
          type="button"
          className={iconButton}
          disabled={disabled || !catalog}
          aria-label={translate('settings.connAdd')}
          title={translate('settings.connAdd')}
          onClick={() => setEditing('new')}
        >
          <Plus size={14} />
          <span className="hidden sm:inline">
            {translate('settings.connNew')}
          </span>
        </button>
        {failed && (
          <span className="flex shrink-0 items-center">
            {unsupported ? (
              <ErrorMark kind="unavailable" message={error} />
            ) : (
              <ErrorMark
                message={error}
                onRetry={() => void reload()}
                retryLabel={translate('settings.connRetry')}
              />
            )}
          </span>
        )}
      </div>
      {!!connectionId && (
        <ConnectionModelField
          value={model}
          onChange={(next) => onChange({ connectionId, model: next })}
          canDiscover={!!selected}
          discoverKey={(selected?.id ?? '') + (selected?.revision ?? '')}
          discover={(signal) => api.models({ id: connectionId }, signal)}
          disabled={disabled}
        />
      )}
      {!!remote.length && !!deviceId && (
        <div className="rounded-lg border border-dashed border-[var(--theme-border)] p-2">
          <p className="px-1 text-xs text-[var(--theme-fg-muted)]">
            {translate('settings.connPickerRemote')}
          </p>
          <ul className="mt-1 space-y-1">
            {remote.map((c) => {
              const sources = syncSources(c, deviceId);
              const state = connectionState(c, deviceId);
              return (
                <li key={c.id} className="px-1">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="min-w-0 flex-1 truncate text-xs">
                      {c.name}
                      <span className="text-[var(--theme-fg-muted)]">
                        {' · '}
                        {state === 'offline'
                          ? translate('settings.connStateOffline')
                          : remoteSources(c, deviceId)
                              .map((s) => s.name)
                              .join(', ')}
                      </span>
                    </span>
                    <button
                      type="button"
                      className={iconButton}
                      disabled={disabled || syncing === c.id || !sources.length}
                      title={translate('settings.connSyncHint')}
                      aria-expanded={
                        sources.length > 1 ? choosing === c.id : undefined
                      }
                      onClick={() => {
                        setNotice('');
                        // Differing versions need an explicit source; one is one click.
                        if (sources.length > 1)
                          setChoosing(choosing === c.id ? null : c.id);
                        else void syncFrom(c, sources[0]!);
                      }}
                    >
                      <CloudDownload size={14} />
                      {syncing === c.id
                        ? translate('settings.connSyncing')
                        : translate('settings.connSync')}
                    </button>
                    {syncErrors[c.id] && syncing !== c.id && (
                      <ErrorMark
                        message={syncErrors[c.id]!}
                        onRetry={
                          sources.length === 1
                            ? () => void syncFrom(c, sources[0]!)
                            : undefined
                        }
                      />
                    )}
                  </div>
                  {choosing === c.id && (
                    <div
                      role="group"
                      aria-label={translate('settings.connChooseSource')}
                      className="mt-1 flex flex-wrap items-center gap-1.5"
                    >
                      <span className="text-xs text-[var(--theme-fg-muted)]">
                        {translate('settings.connChooseSource')}
                      </span>
                      {sources.map((s) => (
                        <button
                          key={s.deviceId}
                          type="button"
                          className={iconButton}
                          disabled={disabled || syncing === c.id}
                          onClick={() => void syncFrom(c, s)}
                        >
                          {s.name}
                          <span className="font-mono text-[var(--theme-fg-muted)]">
                            {s.revision.slice(0, 8)}
                          </span>
                        </button>
                      ))}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      )}
      {notice && (
        <p role="status" className="text-xs text-[var(--theme-accent-strong)]">
          {notice}
        </p>
      )}
      {editing && (
        <ConnectionDialog
          apiRoot={apiRoot}
          connection={editing === 'new' ? null : editing}
          protocols={protocols}
          onClose={() => setEditing(null)}
          onSaved={(saved) => {
            upsert(saved);
            setEditing(null);
            if (editing === 'new')
              onChange({
                connectionId: saved.id,
                model: saved.model || model,
              });
            void reload();
          }}
        />
      )}
    </div>
  );
}
