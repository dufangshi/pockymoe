// Provider presets come from CC Switch via upstreamPresets.ts.
// Copyright (c) 2025 Jason Young. MIT; see THIRD_PARTY_NOTICES.md.
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { FlaskConical } from 'lucide-react';
import { useEffect, useId, useMemo, useState } from 'react';
import { ApiError } from '../lib/api';
import {
  connectionErrorMessage,
  connectionProtocols,
  connectionsApi,
  errorCode,
  protocolLabel,
  type ConnectionDraft,
  type ConnectionProtocol,
  type ConnectionSummary,
} from '../lib/connections';
import { ConnectionModelField } from './ConnectionModelField';
import { upstreamPresets } from './upstreamPresets';

const field =
  'host-input mt-1 min-h-10 w-full min-w-0 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm sm:min-h-9';
const label = 'block text-xs font-medium text-[var(--theme-fg-soft)]';
const presetProtocol: Record<string, ConnectionProtocol> = {
  codex: 'openai_responses',
  claude: 'anthropic',
  gemini: 'gemini',
};

type Preset = {
  name: string;
  protocol: ConnectionProtocol;
  baseUrl: string;
  model: string;
  authType: 'api_key' | 'bearer';
};
export const connectionPresets: Preset[] = [
  ...new Map(
    upstreamPresets
      .filter((p) => presetProtocol[p.harness] && p.baseUrl)
      .map((p) => {
        const protocol = presetProtocol[p.harness]!;
        return [
          `${protocol} ${p.baseUrl}`,
          {
            name: p.name,
            protocol,
            baseUrl: p.baseUrl,
            model: p.model,
            authType: p.authType === 'bearer' ? 'bearer' : 'api_key',
          } satisfies Preset,
        ] as const;
      }),
  ).values(),
];

type Form = {
  name: string;
  protocol: ConnectionProtocol;
  baseUrl: string;
  apiKey: string;
  authType: 'api_key' | 'bearer';
  model: string;
  headers: string | null;
};

function fromSummary(
  c: ConnectionSummary | null | undefined,
  protocol: ConnectionProtocol,
): Form {
  return {
    name: c?.name ?? '',
    protocol: c?.protocol ?? protocol,
    baseUrl: c?.baseUrl ?? '',
    apiKey: '',
    authType: c?.authType ?? 'api_key',
    model: c?.model ?? '',
    headers: null,
  };
}

/** `Name: value` per line; null means the line is invalid. */
export function parseHeaders(text: string) {
  const headers: Record<string, string> = {};
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    const index = line.indexOf(':');
    if (index < 1) return null;
    const name = line.slice(0, index).trim();
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name)) return null;
    headers[name] = line.slice(index + 1).trim();
  }
  return headers;
}

/**
 * The single add/edit form for an upstream connection. Saving stores the connection on
 * this device only; it never applies the connection to a harness or plugin.
 */
export function ConnectionForm({
  apiRoot,
  initial,
  protocols = connectionProtocols,
  onSaved,
  onCancel,
  onDirtyChange,
}: {
  apiRoot: string;
  initial?: ConnectionSummary | null;
  protocols?: ConnectionProtocol[];
  onSaved: (connection: ConnectionSummary) => void;
  onCancel: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  useI18n();
  const id = useId();
  const api = useMemo(() => connectionsApi(apiRoot), [apiRoot]);
  const allowed = protocols.length ? protocols : connectionProtocols;
  const [base, setBase] = useState(initial ?? null);
  const pristine = useMemo(
    () => fromSummary(base, allowed[0]!),
    [base, allowed.join()],
  );
  const [form, setForm] = useState<Form>(pristine);
  const [preset, setPreset] = useState('');
  const [busy, setBusy] = useState<'save' | 'test' | 'reload' | null>(null);
  const [error, setError] = useState('');
  const [conflict, setConflict] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; text: string } | null>(null);
  const saved = !!base?.id;
  // The saved key is reused only for the same URL and wire format.
  const endpointChanged =
    !saved ||
    form.baseUrl.trim() !== (base?.baseUrl ?? '') ||
    form.protocol !== base?.protocol;
  const needsKey = endpointChanged || !base?.hasApiKey;
  const headers =
    form.headers === null ? undefined : parseHeaders(form.headers);
  const urlValid = /^https?:\/\/\S+$/i.test(form.baseUrl.trim());
  const dirty = (Object.keys(form) as (keyof Form)[]).some(
    (key) => form[key] !== pristine[key],
  );
  const protocolOutside = !allowed.includes(form.protocol);
  const valid =
    !!form.name.trim() &&
    urlValid &&
    (!needsKey || !!form.apiKey) &&
    headers !== null &&
    !protocolOutside;

  useEffect(() => onDirtyChange?.(dirty), [dirty]);

  function update(patch: Partial<Form>) {
    setForm((current) => ({ ...current, ...patch }));
    setTest(null);
    if (!conflict) setError('');
  }

  function draft(): ConnectionDraft {
    return {
      ...(base?.id ? { id: base.id, revision: base.revision } : {}),
      name: form.name.trim(),
      protocol: form.protocol,
      baseUrl: form.baseUrl.trim(),
      ...(form.apiKey ? { apiKey: form.apiKey } : {}),
      ...(form.protocol === 'anthropic' ? { authType: form.authType } : {}),
      model: form.model.trim(),
      ...(headers ? { headers } : {}),
    };
  }

  async function save() {
    if (!valid || busy) return;
    setBusy('save');
    setError('');
    try {
      onSaved(await api.save(draft()));
    } catch (e) {
      const code = errorCode(e);
      setConflict(
        code === 'edit_conflict' ||
          (e instanceof ApiError && e.statusCode === 409 && !code),
      );
      setError(connectionErrorMessage(e, translate('settings.connSaveFailed')));
    } finally {
      setBusy(null);
    }
  }

  async function reloadLatest() {
    if (!base?.id) return;
    setBusy('reload');
    try {
      const latest = (await api.list()).connections.find(
        (c) => c.id === base.id && c.local,
      );
      if (!latest) throw Error(translate('settings.connGone'));
      setBase(latest);
      setForm(fromSummary(latest, allowed[0]!));
      setConflict(false);
      setError('');
    } catch (e) {
      setError(connectionErrorMessage(e, translate('settings.connLoadFailed')));
    } finally {
      setBusy(null);
    }
  }

  async function runTest() {
    setBusy('test');
    setTest(null);
    try {
      const result =
        saved && !dirty
          ? await api.test({ id: base!.id, model: form.model.trim() })
          : await api.test(draft());
      setTest({
        ok: result.ok !== false,
        text:
          result.ok === false
            ? translate('settings.connTestFailed')
            : typeof result.latencyMs === 'number'
              ? translate('settings.connTestOkLatency', {
                  ms: result.latencyMs,
                })
              : translate('settings.connTestOk'),
      });
    } catch (e) {
      setTest({
        ok: false,
        text:
          e instanceof ApiError && [404, 405].includes(e.statusCode) && dirty
            ? translate('settings.connTestSaveFirst')
            : connectionErrorMessage(e, translate('settings.connTestFailed')),
      });
    } finally {
      setBusy(null);
    }
  }

  const discoverKey = JSON.stringify([
    form.protocol,
    form.baseUrl.trim(),
    form.apiKey,
    form.authType,
    form.headers,
  ]);
  const canDiscover = urlValid && (!needsKey || !!form.apiKey);
  return (
    <form
      className="space-y-3"
      noValidate
      onSubmit={(e) => {
        // The form can be rendered from inside another form through a portal.
        e.preventDefault();
        e.stopPropagation();
        void save();
      }}
    >
      {!saved && (
        <label className={label}>
          {translate('settings.connPreset')}
          <select
            className={field}
            value={preset}
            onChange={(e) => {
              setPreset(e.target.value);
              const p = connectionPresets[Number(e.target.value)];
              if (e.target.value === '' || !p) return;
              update({
                name: p.name,
                protocol: p.protocol,
                baseUrl: p.baseUrl,
                model: p.model,
                authType: p.authType,
              });
            }}
          >
            <option value="">{translate('settings.connPresetCustom')}</option>
            {connectionPresets.map((p, i) =>
              allowed.includes(p.protocol) ? (
                <option key={i} value={i}>
                  {p.name} · {protocolLabel(p.protocol)}
                </option>
              ) : null,
            )}
          </select>
        </label>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <label className={label}>
          {translate('settings.connName')}
          <input
            className={field}
            required
            value={form.name}
            autoComplete="off"
            onChange={(e) => update({ name: e.target.value })}
          />
        </label>
        <label className={label}>
          {translate('settings.connProtocol')}
          <select
            className={field}
            value={form.protocol}
            aria-describedby={`${id}-protocol`}
            onChange={(e) =>
              update({ protocol: e.target.value as ConnectionProtocol })
            }
          >
            {connectionProtocols
              .filter((p) => allowed.includes(p) || p === form.protocol)
              .map((p) => (
                <option key={p} value={p} disabled={!allowed.includes(p)}>
                  {protocolLabel(p)}
                </option>
              ))}
          </select>
        </label>
      </div>
      <p
        id={`${id}-protocol`}
        className={`-mt-1 text-xs ${protocolOutside ? 'text-[var(--status-danger-fg)]' : 'text-[var(--theme-fg-muted)]'}`}
      >
        {translate(
          protocolOutside
            ? 'settings.connProtocolNotHere'
            : 'settings.connProtocolHint',
        )}
      </p>
      <label className={label}>
        {translate('settings.connBaseUrl')}
        <input
          className={field}
          type="url"
          inputMode="url"
          required
          placeholder="https://api.example.com/v1"
          value={form.baseUrl}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => update({ baseUrl: e.target.value })}
        />
      </label>
      <div
        className={
          form.protocol === 'anthropic' ? 'grid gap-3 sm:grid-cols-2' : ''
        }
      >
        <div>
          <label className={label}>
            {translate('settings.connApiKey')}
            <input
              className={field}
              type="password"
              autoComplete="new-password"
              required={needsKey}
              placeholder={needsKey ? '' : translate('settings.connApiKeyKeep')}
              value={form.apiKey}
              aria-describedby={`${id}-key`}
              onChange={(e) => update({ apiKey: e.target.value })}
            />
          </label>
          {saved && base?.hasApiKey && endpointChanged && (
            <p
              id={`${id}-key`}
              className="mt-1 text-xs text-[var(--theme-fg-muted)]"
            >
              {translate('settings.connApiKeyEndpointChanged')}
            </p>
          )}
        </div>
        {form.protocol === 'anthropic' && (
          <label className={label}>
            {translate('settings.connAuth')}
            <select
              className={field}
              value={form.authType}
              onChange={(e) =>
                update({ authType: e.target.value as Form['authType'] })
              }
            >
              <option value="api_key">
                {translate('settings.connAuthKey')}
              </option>
              <option value="bearer">
                {translate('settings.connAuthBearer')}
              </option>
            </select>
          </label>
        )}
      </div>
      <ConnectionModelField
        value={form.model}
        onChange={(model) => update({ model })}
        canDiscover={canDiscover}
        discoverKey={discoverKey}
        discover={(signal) => api.models(draft(), signal)}
      />
      <details className="text-xs" open={form.headers !== null || undefined}>
        <summary className="cursor-pointer py-1 text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.connAdvanced')}
        </summary>
        <label className={`mt-2 ${label}`}>
          {translate('settings.connHeaders')}
          <textarea
            className={`${field} font-mono text-xs`}
            rows={3}
            spellCheck={false}
            value={form.headers ?? ''}
            placeholder={
              base?.headers && Object.keys(base.headers).length
                ? translate('settings.connHeadersStored', {
                    names: Object.keys(base.headers).join(', '),
                  })
                : 'X-Example: value'
            }
            onChange={(e) => update({ headers: e.target.value })}
          />
        </label>
        {headers === null && (
          <p
            role="alert"
            className="mt-1 text-xs text-[var(--status-danger-fg)]"
          >
            {translate('settings.connHeadersInvalid')}
          </p>
        )}
      </details>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="host-secondary-button inline-flex min-h-10 items-center gap-1.5 sm:min-h-9 rounded-lg border px-3 text-xs disabled:opacity-40"
          disabled={!!busy || !canDiscover || headers === null}
          onClick={() => void runTest()}
        >
          <FlaskConical size={14} />
          {busy === 'test'
            ? translate('settings.connTesting')
            : translate('settings.connTest')}
        </button>
        {test && (
          <span
            role={test.ok ? 'status' : 'alert'}
            className={`min-w-0 break-words text-xs ${test.ok ? 'text-[var(--theme-accent-strong)]' : 'text-[var(--status-danger-fg)]'}`}
          >
            {test.text}
          </span>
        )}
      </div>
      {error && (
        <div
          role="alert"
          className="rounded-lg border border-[var(--status-danger-border,var(--theme-border))] p-3 text-xs text-[var(--status-danger-fg)]"
        >
          <p className="break-words">{error}</p>
          {conflict && saved && (
            <button
              type="button"
              className="host-secondary-button mt-2 min-h-9 rounded-lg border px-3 text-xs text-[var(--theme-fg)]"
              disabled={!!busy}
              onClick={() => void reloadLatest()}
            >
              {translate('settings.connReloadLatest')}
            </button>
          )}
        </div>
      )}
      {/* One row on every width; the save-only hint sits under the dialog title.
          Sticky offsets start inside the dialog's 24px padding, so reach through it. */}
      <div
        className="sticky -bottom-6 -mx-1 -mb-6 grid grid-cols-2 gap-2 border-t border-[var(--theme-border)] bg-[var(--theme-panel)] px-1 pb-6 pt-2.5 sm:flex sm:justify-end"
        data-testid="connection-form-actions"
      >
        <button
          type="button"
          className="host-secondary-button min-h-10 whitespace-nowrap rounded-lg border px-4 text-[13px] sm:min-h-9"
          disabled={busy === 'save'}
          onClick={onCancel}
        >
          {translate('settings.connCancel')}
        </button>
        <button
          type="submit"
          className="relay-button-primary !min-h-10 whitespace-nowrap !px-4 sm:!min-h-9"
          aria-label={translate('settings.connSave')}
          disabled={!valid || !!busy || (saved && !dirty)}
        >
          {busy === 'save'
            ? translate('settings.connSaving')
            : translate('settings.connSaveShort')}
        </button>
      </div>
    </form>
  );
}
