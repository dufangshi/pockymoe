import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { useEffect, useId, useState } from 'react';
import { useLocation } from 'react-router-dom';
import type { HarnessResourceSettingsDto } from '@pockymoe/shared';
import {
  ApiError,
  fetchHarnessResourceSettings,
  relayModeActive,
  updateHarnessResourceSettings,
} from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { gibText, parseResourceField, RESOURCE_LIMITS, type ResourceField as Field } from './harnessResourceModel';

type Draft = Record<Field, { on: boolean; text: string }>;

const FIELDS: Array<{
  field: Field;
  label: TranslationKey;
  hint: TranslationKey;
  range: TranslationKey;
  unit: () => string;
  fallback: string;
  step: string;
}> = [
  { field: 'idleReleaseMinutes', label: 'settings.idleRelease', hint: 'settings.idleReleaseHint', range: 'settings.resourcesMinutesRange', unit: () => translate('settings.idleReleaseUnit'), fallback: '10', step: '1' },
  { field: 'maxLoadedThreads', label: 'settings.maxLoadedThreads', hint: 'settings.maxLoadedThreadsHint', range: 'settings.resourcesThreadsRange', unit: () => '', fallback: '10', step: '1' },
  { field: 'maxHarnessMemoryBytes', label: 'settings.maxHarnessMemory', hint: 'settings.maxHarnessMemoryHint', range: 'settings.resourcesMemoryRange', unit: () => 'GiB', fallback: '4', step: '0.25' },
];

function toDraft(settings: HarnessResourceSettingsDto): Draft {
  const entry = (value: number | null, fallback: string, text: (value: number) => string) =>
    ({ on: value != null, text: value == null ? fallback : text(value) });
  return {
    idleReleaseMinutes: entry(settings.idleReleaseMinutes, '10', String),
    maxLoadedThreads: entry(settings.maxLoadedThreads, '10', String),
    maxHarnessMemoryBytes: entry(settings.maxHarnessMemoryBytes, '4', gibText),
  };
}

export function HarnessResourceSettings() {
  useI18n();
  const { pathname } = useLocation();
  const deviceId = relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) {
    return <p className="py-5 text-sm text-[var(--theme-fg-muted)]">{translate('settings.resourcesChooseDevice')}</p>;
  }
  return <DeviceResourceSettings key={deviceId ?? 'local'} deviceId={deviceId} />;
}

function DeviceResourceSettings({ deviceId }: { deviceId: string | null }) {
  const scope = useId();
  const [saved, setSaved] = useState<HarnessResourceSettingsDto | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let alive = true;
    setError(null);
    fetchHarnessResourceSettings(deviceId).then(settings => {
      if (!alive) return;
      setSaved(settings);
      setDraft(toDraft(settings));
    }).catch((cause: unknown) => {
      if (!alive) return;
      if (cause instanceof ApiError && cause.statusCode === 404) setUnsupported(true);
      else setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { alive = false; };
  }, [deviceId, revision]);

  if (unsupported)
    return <p className="py-5 text-sm text-[var(--theme-fg-muted)]">{translate('settings.resourcesUnavailable')}</p>;
  if (!draft) {
    return error ? (
      <div className="py-5">
        <p className="host-error rounded-md border px-3 py-2 text-xs" role="alert">{error}</p>
        <button type="button" className="host-secondary-button mt-2 min-h-9 rounded-md border px-3 text-xs" onClick={() => setRevision(value => value + 1)}>
          {translate('files.retry')}
        </button>
      </div>
    ) : <p className="py-5 text-sm text-[var(--theme-fg-muted)]">{translate('settings.loading')}</p>;
  }

  const values = Object.fromEntries(FIELDS.map(({ field }) =>
    [field, parseResourceField(field, draft[field].on, draft[field].text, saved?.[field])])) as Record<Field, number | null | undefined>;
  const invalid = FIELDS.some(({ field }) => values[field] === undefined);
  const change = (field: Field, patch: Partial<Draft[Field]>) => {
    setDraft(current => current && { ...current, [field]: { ...current[field], ...patch } });
    setMessage(null);
    setError(null);
  };
  const save = async () => {
    if (invalid || busy) return;
    setBusy(true);
    setMessage(null);
    setError(null);
    try {
      const next = await updateHarnessResourceSettings(values as HarnessResourceSettingsDto, deviceId);
      setSaved(next);
      setDraft(toDraft(next));
      setMessage(translate('settings.resourcesSaved'));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="grid gap-4 py-4">
      {FIELDS.map(({ field, label, hint, range, unit, fallback, step }) => {
        const entry = draft[field];
        const bad = values[field] === undefined;
        const id = `${scope}-${field}`;
        return (
          <div key={field}>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <label className="flex min-w-0 flex-1 items-center gap-2 text-sm font-medium text-[var(--theme-fg)]">
                <input type="checkbox" checked={entry.on} disabled={busy}
                  onChange={event => change(field, { on: event.target.checked, ...(entry.text ? {} : { text: fallback }) })} />
                {translate(label)}
              </label>
              <span className="flex items-center gap-1.5 text-xs text-[var(--theme-fg-muted)]">
                <input id={id} type="number" inputMode="decimal" step={step} min={field === 'maxHarnessMemoryBytes' ? 0.25 : RESOURCE_LIMITS[field].min}
                  aria-label={translate(label)} aria-invalid={bad || undefined} aria-describedby={`${id}-hint`}
                  disabled={!entry.on || busy} value={entry.text}
                  onChange={event => change(field, { text: event.target.value })}
                  className="relay-input min-h-9 w-24 rounded-md text-right disabled:opacity-50" />
                {unit()}
              </span>
            </div>
            <p id={`${id}-hint`} className={`settings-hint mt-1 text-xs leading-5 ${bad ? 'text-[var(--status-danger-fg)]' : 'text-[var(--theme-fg-muted)]'}`}>
              {translate(bad ? range : hint)}
            </p>
          </div>
        );
      })}
      <div className="flex items-center gap-3">
        <button type="button" className="relay-button-primary min-h-10 rounded-md px-4" disabled={invalid || busy} onClick={() => void save()}>
          {busy ? translate('settings.saving') : translate('files.save')}
        </button>
        {message && <span role="status" className="text-xs text-[var(--status-success-fg)]">{message}</span>}
      </div>
      {error && <p className="host-error rounded-md border px-3 py-2 text-xs" role="alert">{error}</p>}
    </section>
  );
}
