import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useEffect, useState } from 'react';
import { request } from '../lib/api';

export function UpstreamModelPicker({
  apiRoot,
  connection,
  value,
  onChange,
  required = true,
}: {
  apiRoot: string;
  connection: {
    id: string;
    harness: string;
    baseUrl: string;
    apiKey?: string;
    authType?: string;
    hasApiKey?: boolean;
    settingsConfig?: Record<string, unknown>;
    apiType?: string;
  };
  required?: boolean;
  value: string;
  onChange: (model: string) => void;
}) {
  useI18n();
  const [models, setModels] = useState<{ id: string; name: string }[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [truncated, setTruncated] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const { id, harness, baseUrl, apiKey, authType, hasApiKey } = connection;
  const configKey = JSON.stringify(connection.settingsConfig ?? {});
  const ready =
    (baseUrl.startsWith('https://') || baseUrl.startsWith('http://')) &&
    !!(apiKey || (id && hasApiKey));
  useEffect(() => {
    if (!ready) return;
    let alive = true;
    const controller = new AbortController();
    setLoading(true);
    setError('');
    const timer = window.setTimeout(() => {
      void request<{
        models: { id: string; name: string }[];
        truncated: boolean;
      }>(`${apiRoot}/management/upstreams/models`, {
        method: 'POST',
        signal: controller.signal,
        body: JSON.stringify({
          id,
          harness,
          baseUrl,
          apiKey: apiKey ?? '',
          authType: authType ?? 'api_key',
          settingsConfig: connection.settingsConfig ?? {},
          apiType: connection.apiType ?? 'responses',
        }),
      })
        .then((result) => {
          if (!alive) return;
          if (!Array.isArray(result.models))
            throw Error(
              translate('settings.updateThisSupervisorToEnableModelDiscovery'),
            );
          setModels(result.models);
          setTruncated(result.truncated);
        })
        .catch((e) => {
          if (alive)
            setError(
              e instanceof Error
                ? e.message
                : translate('settings.unableToLoadModels'),
            );
        })
        .finally(() => {
          if (alive) setLoading(false);
        });
    }, 400);
    return () => {
      alive = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [
    apiRoot,
    id,
    harness,
    baseUrl,
    apiKey,
    authType,
    ready,
    refresh,
    configKey,
    connection.apiType,
  ]);
  return (
    <div>
      <label className="block text-sm">
        {translate('settings.model')}
        <select
          className="host-input mt-1 w-full min-w-0 rounded-md border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2 text-sm"
          required={required}
          value={value}
          disabled={!ready || loading}
          onChange={(e) => onChange(e.target.value)}
        >
          <option value="">
            {loading
              ? translate('settings.loadingModels')
              : translate('settings.selectAModel')}
          </option>
          {value && !models.some((m) => m.id === value) && (
            <option value={value}>
              {value} {translate('settings.configured')}
            </option>
          )}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name === m.id ? m.id : `${m.name} · ${m.id}`}
            </option>
          ))}
        </select>
      </label>
      <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-[var(--theme-fg-muted)]">
        <span>
          {!ready
            ? translate('settings.enterTheUpstreamURLAndAPIKey')
            : loading
              ? translate('settings.discoveringModelsFromThisUpstream')
              : error
                ? ''
                : models.length
                  ? translate('settings.modelsFound', {
                      value1: models.length,
                      value2: truncated
                        ? translate('settings.listTruncated')
                        : '',
                    })
                  : translate('settings.noModelsReturnedByThisUpstream')}
        </span>
        <button
          type="button"
          className="min-h-9 rounded-md border border-[var(--theme-border)] px-3 disabled:opacity-40"
          disabled={!ready || loading}
          onClick={() => setRefresh((n) => n + 1)}
        >
          {translate('settings.refreshModels')}
        </button>
        {/* Discovery failed: the reason and a Retry on the mark after Refresh. */}
        {error && !loading && (
          <ErrorMark message={error} onRetry={() => setRefresh((n) => n + 1)} />
        )}
      </div>
    </div>
  );
}
