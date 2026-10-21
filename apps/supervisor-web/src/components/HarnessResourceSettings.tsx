import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { useEffect, useId, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { ErrorMark } from '@pockymoe/thread-ui';
import type { HarnessResourceSettingsDto } from '@pockymoe/shared';
import {
  ApiError,
  fetchHarnessResourceSettings,
  relayModeActive,
  updateHarnessResourceSettings,
} from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { gibText, parseResourceField, RESOURCE_LIMITS, type ResourceField as Field } from './harnessResourceModel';
import { StorageSettings } from './StorageSettings';
import './ResourceSettings.css';

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
  return (
    <div className="pm-resources">
      <section className="pm-res-section" aria-labelledby="pm-res-memory">
        <h3 id="pm-res-memory" className="pm-res-heading">{translate('settings.resourcesMemory')}</h3>
        <DeviceResourceSettings key={deviceId ?? 'local'} deviceId={deviceId} />
      </section>
      <section className="pm-res-section" aria-labelledby="pm-res-disk">
        <h3 id="pm-res-disk" className="pm-res-heading">{translate('settings.resourcesDisk')}</h3>
        <StorageSettings key={deviceId ?? 'local'} deviceId={deviceId} />
      </section>
    </div>
  );
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
      // Nothing could be read: an obvious Retry, and the reason on its mark.
      <div className="flex items-center gap-1 py-5">
        <button type="button" className="host-secondary-button min-h-9 rounded-md border px-3 text-xs" onClick={() => setRevision(value => value + 1)}>
          {translate('files.retry')}
        </button>
        <ErrorMark message={error} />
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
    <div className="pm-res-card">
      {FIELDS.map(({ field, label, hint, range, unit, fallback, step }) => {
        const entry = draft[field];
        const bad = values[field] === undefined;
        const id = `${scope}-${field}`;
        return (
          <div key={field} className="pm-res-row" data-off={!entry.on || undefined}>
            <label className="pm-res-label">
              <input type="checkbox" checked={entry.on} disabled={busy}
                onChange={event => change(field, { on: event.target.checked, ...(entry.text ? {} : { text: fallback }) })} />
              <span>{translate(label)}</span>
            </label>
            <span className="pm-res-value">
              <input id={id} type="number" inputMode="decimal" step={step} min={field === 'maxHarnessMemoryBytes' ? 0.25 : RESOURCE_LIMITS[field].min}
                aria-label={translate(label)} aria-invalid={bad || undefined} aria-describedby={`${id}-hint`}
                disabled={!entry.on || busy} value={entry.text}
                onChange={event => change(field, { text: event.target.value })}
                className="relay-input pm-res-input" />
              <span className="pm-res-unit">{unit()}</span>
            </span>
            <p id={`${id}-hint`} className={`pm-res-hint${bad ? ' is-error' : ''}`}>{translate(bad ? range : hint)}</p>
          </div>
        );
      })}
      <div className="pm-res-footer">
        {message && <span role="status" className="pm-res-saved">{message}</span>}
        <button type="button" className="relay-button-primary pm-res-primary" disabled={invalid || busy} onClick={() => void save()}>
          {busy ? translate('settings.saving') : translate('files.save')}
        </button>
        {error && <ErrorMark message={error} onRetry={invalid ? undefined : () => void save()} />}
      </div>
    </div>
  );
}
