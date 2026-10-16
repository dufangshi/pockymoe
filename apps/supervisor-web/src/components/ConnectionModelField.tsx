import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { RefreshCw } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { DiscoveredModel } from '../lib/connections';

type Discovery = { models: DiscoveredModel[]; truncated?: boolean };

/**
 * A model ID the user can always type, plus an optional list discovered from the
 * upstream. Discovery failure never blocks the manual value.
 */
export function ConnectionModelField({
  value,
  onChange,
  discover,
  canDiscover,
  discoverKey,
  autoDiscover = false,
  disabled = false,
  label = translate('settings.connModel'),
}: {
  value: string;
  onChange: (model: string) => void;
  discover: (signal: AbortSignal) => Promise<Discovery>;
  canDiscover: boolean;
  /** Changing the key forgets an older list, e.g. after editing the endpoint. */
  discoverKey: string;
  autoDiscover?: boolean;
  disabled?: boolean;
  label?: string;
}) {
  useI18n();
  const id = useId();
  const [models, setModels] = useState<DiscoveredModel[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [state, setState] = useState<'idle' | 'loading' | 'error'>('idle');
  const [error, setError] = useState('');
  const controller = useRef<AbortController | null>(null);
  const discoverRef = useRef(discover);
  discoverRef.current = discover;

  function load() {
    controller.current?.abort();
    const current = new AbortController();
    controller.current = current;
    setState('loading');
    setError('');
    discoverRef
      .current(current.signal)
      .then((result) => {
        if (current.signal.aborted) return;
        if (!Array.isArray(result?.models))
          throw Error(translate('settings.connModelsUnsupported'));
        setModels(result.models);
        setTruncated(!!result.truncated);
        setState('idle');
      })
      .catch((e: unknown) => {
        if (current.signal.aborted) return;
        setModels(null);
        setState('error');
        setError(e instanceof Error && e.message ? e.message : '');
      });
  }

  useEffect(() => {
    controller.current?.abort();
    setModels(null);
    setState('idle');
    setError('');
    if (autoDiscover && canDiscover) load();
    return () => controller.current?.abort();
  }, [discoverKey, canDiscover, autoDiscover]);

  const listed = models?.some((m) => m.id === value);
  const status =
    state === 'loading'
      ? translate('settings.connModelsLoading')
      : state === 'error'
        ? translate('settings.connModelsFailed', {
            reason: error || translate('settings.connUnknownError'),
          })
        : models && !models.length
          ? translate('settings.connModelsEmpty')
          : !canDiscover
            ? translate('settings.connModelsNeedEndpoint')
            : '';
  return (
    <div className="min-w-0">
      <label
        className="block text-xs font-medium text-[var(--theme-fg-soft)]"
        htmlFor={`${id}-model`}
      >
        {label}
      </label>
      <div className="mt-1 flex min-w-0 gap-2">
        <input
          id={`${id}-model`}
          className="host-input min-h-9 min-w-0 flex-1 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm"
          value={value}
          disabled={disabled}
          autoComplete="off"
          spellCheck={false}
          placeholder={translate('settings.connModelPlaceholder')}
          aria-describedby={`${id}-status`}
          onChange={(e) => onChange(e.target.value)}
        />
        <button
          type="button"
          className="host-secondary-button inline-flex min-h-9 min-w-9 shrink-0 items-center justify-center gap-1.5 rounded-lg border px-3 text-xs disabled:opacity-40"
          disabled={disabled || !canDiscover || state === 'loading'}
          aria-label={translate(
            models ? 'settings.connRefreshModels' : 'settings.connFindModels',
          )}
          title={translate(
            models ? 'settings.connRefreshModels' : 'settings.connFindModels',
          )}
          onClick={load}
        >
          <RefreshCw
            size={14}
            className={state === 'loading' ? 'animate-spin' : undefined}
          />
          <span className="hidden sm:inline">
            {translate(
              models ? 'settings.connRefreshModels' : 'settings.connFindModels',
            )}
          </span>
        </button>
      </div>
      {!!models?.length && (
        <select
          className="host-input mt-1.5 min-h-9 w-full min-w-0 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-2.5 py-1.5 text-sm"
          aria-label={translate('settings.connChooseModel')}
          disabled={disabled}
          value={listed ? value : ''}
          onChange={(e) => e.target.value && onChange(e.target.value)}
        >
          <option value="">
            {translate('settings.connModelsFound', {
              count: models.length,
              more: truncated ? '+' : '',
            })}
          </option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name && m.name !== m.id ? `${m.name} · ${m.id}` : m.id}
            </option>
          ))}
        </select>
      )}
      {status && (
        <p
          id={`${id}-status`}
          role={state === 'error' ? 'alert' : undefined}
          className={`mt-1 break-words text-xs ${state === 'error' ? 'text-[var(--status-danger-fg)]' : 'text-[var(--theme-fg-muted)]'}`}
        >
          {status}
        </p>
      )}
    </div>
  );
}
