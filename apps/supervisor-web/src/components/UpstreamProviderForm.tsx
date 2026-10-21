// Form sections / preset application adapted from CC Switch providers/forms.
// Copyright (c) 2025 Jason Young. MIT; see THIRD_PARTY_NOTICES.md.
import { useState } from 'react';
import { translate } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { upstreamPresets } from './upstreamPresets';
import { UpstreamModelPicker } from './UpstreamModelPicker';
import type { Profile } from './UpstreamManagement';
const field =
  'host-input mt-1 w-full rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2 text-sm';
const masked = '[stored privately]';
export function UpstreamProviderForm({
  profile: p,
  harnessName,
  apiRoot,
  busy,
  error,
  onChange,
  onSave,
}: {
  profile: Profile;
  harnessName: string;
  apiRoot: string;
  busy: boolean;
  error: string;
  onChange: (p: Profile) => void;
  onSave: (p: Profile) => void;
}) {
  const [preset, setPreset] = useState('');
  const [discover, setDiscover] = useState(false);
  const [raw, setRaw] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const config = p.settingsConfig ?? {};
  const env = (config.env ?? {}) as Record<string, string>;
  function fragment(next: Record<string, unknown>) {
    setRaw(null);
    setInvalid(false);
    onChange({ ...p, settingsConfig: next });
  }
  function envValue(key: string, value: string) {
    const next = { ...env };
    if (value) next[key] = value;
    else delete next[key];
    fragment({ ...config, env: next });
  }
  const presets = upstreamPresets.filter((v) => v.harness === p.harness);
  const headers =
    p.harness === 'claude'
      ? env.ANTHROPIC_CUSTOM_HEADERS
      : p.harness === 'gemini'
        ? env.GEMINI_CLI_CUSTOM_HEADERS
        : p.harness === 'deepseek'
          ? config.headers
          : (config.provider as Record<string, unknown> | undefined)
              ?.http_headers;
  const [headerRaw, setHeaderRaw] = useState<string | null>(null);
  const [headerInvalid, setHeaderInvalid] = useState(false);
  function changeHeaders(value: string) {
    setHeaderRaw(value);
    if (p.harness === 'claude' || p.harness === 'gemini') {
      envValue(
        p.harness === 'claude'
          ? 'ANTHROPIC_CUSTOM_HEADERS'
          : 'GEMINI_CLI_CUSTOM_HEADERS',
        value,
      );
      return;
    }
    try {
      const object = value.trim() ? JSON.parse(value) : {};
      if (
        !object ||
        Array.isArray(object) ||
        typeof object !== 'object' ||
        Object.values(object).some((v) => typeof v !== 'string')
      )
        throw Error();
      setHeaderInvalid(false);
      fragment(
        p.harness === 'deepseek'
          ? { ...config, headers: object }
          : {
              ...config,
              provider: {
                ...((config.provider as object) ?? {}),
                http_headers: object,
              },
            },
      );
    } catch {
      setHeaderInvalid(true);
    }
  }
  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!invalid && !headerInvalid) onSave(p);
      }}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="block text-sm">
          {translate('settings.upstreamsPreset')}
          <select
            className={field}
            value={preset}
            onChange={(e) => {
              setPreset(e.target.value);
              setRaw(null);
              setHeaderRaw(null);
              setInvalid(false);
              setHeaderInvalid(false);
              if (e.target.value === 'dsh-official') {
                onChange({
                  ...p,
                  name: 'DeepSeek',
                  baseUrl: 'https://api.deepseek.com/anthropic',
                  model: 'deepseek-v4-flash',
                  apiType: 'anthropic',
                  settingsConfig: { profile: 'acp' },
                  apiKey: '',
                  hasApiKey: false,
                });
                return;
              }
              if (e.target.value === 'dsh-compatible') {
                onChange({
                  ...p,
                  name: '',
                  baseUrl: '',
                  model: '',
                  apiType: 'chat_completions',
                  settingsConfig: { profile: 'acp' },
                  apiKey: '',
                  hasApiKey: false,
                });
                return;
              }
              const selected = presets[Number(e.target.value)];
              if (e.target.value !== '' && selected)
                onChange({
                  ...p,
                  ...selected,
                  settingsConfig: structuredClone(selected.settingsConfig),
                  apiKey: '',
                  hasApiKey: false,
                });
            }}
          >
            <option value="">{translate('settings.upstreamsCustom')}</option>
            {p.harness === 'deepseek' ? (
              <>
                <option value="dsh-official">
                  {translate('settings.upstreamsDeepSeekOfficial')}
                </option>
                <option value="dsh-compatible">
                  {translate('settings.upstreamsOpenAICompatible')}
                </option>
              </>
            ) : (
              presets.map((v, i) => (
                <option value={i} key={i}>
                  {v.name}
                </option>
              ))
            )}
          </select>
        </label>
        <label className="block text-sm">
          {translate('settings.upstreamsHarness')}
          <select className={field} value={p.harness} disabled>
            <option value={p.harness}>{harnessName}</option>
          </select>
        </label>
      </div>
      <label className="block text-sm">
        {translate('settings.name')}
        <input
          className={field}
          required
          value={p.name}
          onChange={(e) => onChange({ ...p, name: e.target.value })}
        />
      </label>
      <label className="block text-sm">
        {translate('settings.baseURL')}
        <input
          className={field}
          type="url"
          required
          value={p.baseUrl}
          onChange={(e) => onChange({ ...p, baseUrl: e.target.value })}
        />
      </label>
      <label className="block text-sm">
        {translate('settings.aPIKey')}
        <input
          className={field}
          type="password"
          autoComplete="off"
          required={!p.id || !p.hasApiKey}
          placeholder={
            p.id && p.hasApiKey
              ? translate('settings.leaveEmptyToKeepTheSavedKey')
              : ''
          }
          value={p.apiKey ?? ''}
          onChange={(e) => onChange({ ...p, apiKey: e.target.value })}
        />
      </label>
      {p.harness === 'claude' && (
        <label className="block text-sm">
          {translate('settings.authentication')}
          <select
            className={field}
            value={p.authType ?? 'api_key'}
            onChange={(e) => onChange({ ...p, authType: e.target.value })}
          >
            <option value="api_key">
              {translate('settings.aPIKeyXApiKey')}
            </option>
            <option value="bearer">{translate('settings.bearerToken')}</option>
          </select>
        </label>
      )}
      <label className="block text-sm">
        {translate('settings.upstreamsModelId')}
        <input
          className={field}
          required
          value={p.model}
          onChange={(e) => onChange({ ...p, model: e.target.value })}
        />
      </label>
      <details onToggle={(e) => setDiscover(e.currentTarget.open)}>
        <summary className="cursor-pointer text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.upstreamsDiscoverModels')}
        </summary>
        <div className="mt-3">
          {discover && (
            <UpstreamModelPicker
              required={false}
              apiRoot={apiRoot}
              connection={p}
              value={p.model}
              onChange={(model) => onChange({ ...p, model })}
            />
          )}
        </div>
      </details>
      {p.harness === 'claude' && (
        <div className="grid gap-3 sm:grid-cols-3">
          {[
            [
              'ANTHROPIC_DEFAULT_HAIKU_MODEL',
              translate('settings.upstreamsHaiku'),
            ],
            [
              'ANTHROPIC_DEFAULT_SONNET_MODEL',
              translate('settings.upstreamsSonnet'),
            ],
            [
              'ANTHROPIC_DEFAULT_OPUS_MODEL',
              translate('settings.upstreamsOpus'),
            ],
          ].map(([key = '', label = '']) => (
            <label key={key} className="block text-sm">
              {label}
              <input
                className={field}
                value={env[key] ?? ''}
                onChange={(e) => envValue(key, e.target.value)}
              />
            </label>
          ))}
        </div>
      )}
      {['grok', 'deepseek'].includes(p.harness) && (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="block text-sm">
            {translate('settings.aPIFormat')}
            <select
              className={field}
              value={p.apiType}
              onChange={(e) => onChange({ ...p, apiType: e.target.value })}
            >
              {p.harness === 'deepseek' && (
                <option value="anthropic">
                  {translate('settings.upstreamsMessages')}
                </option>
              )}
              <option value="responses">
                {translate('settings.upstreamsResponses')}
              </option>
              <option value="chat_completions">
                {translate('settings.chatCompletions')}
              </option>
            </select>
          </label>
          <label className="block text-sm">
            {translate('settings.contextWindow')}
            <input
              type="number"
              min="1"
              className={field}
              value={p.contextWindow}
              onChange={(e) =>
                onChange({ ...p, contextWindow: Number(e.target.value) })
              }
            />
          </label>
        </div>
      )}
      {p.harness !== 'grok' &&
        !(p.harness === 'deepseek' && p.apiType === 'anthropic') && (
          <label className="block text-sm">
            {translate('settings.upstreamsHeaders')}
            <textarea
              className={`${field} font-mono`}
              rows={2}
              value={
                headerRaw ??
                (typeof headers === 'string'
                  ? headers === masked
                    ? ''
                    : headers
                  : headers
                    ? JSON.stringify(headers, null, 2)
                    : '')
              }
              placeholder={
                typeof headers === 'string' && headers === masked
                  ? translate('settings.upstreamsStoredHeaders')
                  : translate('settings.upstreamsHeadersHint')
              }
              onChange={(e) => changeHeaders(e.target.value)}
            />
          </label>
        )}
      {p.harness !== 'grok' && (
        <>
          <details>
            <summary className="cursor-pointer text-sm font-medium">
              {translate('settings.upstreamsConfigFragment')}
            </summary>
            <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
              {translate('settings.upstreamsConfigHint')}
            </p>
            <textarea
              aria-label={translate('settings.upstreamsConfigFragment')}
              className={`${field} font-mono`}
              rows={8}
              spellCheck={false}
              value={raw ?? JSON.stringify(config, null, 2)}
              onChange={(e) => {
                setRaw(e.target.value);
                try {
                  const next = JSON.parse(e.target.value);
                  if (!next || typeof next !== 'object' || Array.isArray(next))
                    throw Error();
                  setInvalid(false);
                  onChange({ ...p, settingsConfig: next });
                } catch {
                  setInvalid(true);
                }
              }}
            />
          </details>
        </>
      )}
      {(invalid || headerInvalid) && (
        <p role="alert" className="text-xs text-[var(--status-danger-fg)]">
          {translate('settings.upstreamsInvalidJSON')}
        </p>
      )}
      <div className="sticky bottom-0 flex items-center justify-end gap-2 border-t border-[var(--theme-border)] bg-[var(--theme-panel)] pt-3">
        <button
          className="relay-button-primary min-h-10"
          type="submit"
          disabled={busy || invalid || headerInvalid || !p.model}
        >
          {busy
            ? translate('settings.saving_56a228')
            : translate('settings.saveUpstream')}
        </button>
        {error && !busy && <ErrorMark message={error} />}
      </div>
    </form>
  );
}
