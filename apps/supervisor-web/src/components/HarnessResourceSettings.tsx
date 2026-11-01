import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { ErrorMark } from '@pockymoe/thread-ui';
import type { HarnessResourceSettingsDto } from '@pockymoe/shared';
import {
  ApiError,
  fetchContextInspectionSettings,
  fetchHarnessResourceSettings,
  relayModeActive,
  updateContextInspectionSettings,
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
      <section className="pm-res-section" aria-labelledby="pm-res-diagnostics">
        <h3 id="pm-res-diagnostics" className="pm-res-heading">{translate('settings.contextInspectionHeading')}</h3>
        <ContextInspectionSetting key={deviceId ?? 'local'} deviceId={deviceId} />
      </section>
    </div>
  );
}

/** A preference of this device (docs/ui-principles.md §15): it saves as it changes,
    says so in place, and goes back to the saved value when saving fails. It stays
    enabled while saving, so keyboard focus is not dropped from the switch. */
export function ContextInspectionSetting({ deviceId }: { deviceId: string | null }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<{ message: string; retry: () => void } | null>(null);
  const saving = useRef(false);
  const load = useCallback(() => {
    let alive = true;
    setError(null);
    fetchContextInspectionSettings(deviceId).then(settings => {
      if (alive) setEnabled(settings.enabled);
    }).catch((cause: unknown) => {
      if (!alive) return;
      if (cause instanceof ApiError && cause.statusCode === 404) setUnsupported(true);
      else setError({ message: cause instanceof Error ? cause.message : String(cause), retry: load });
    });
    return () => { alive = false; };
  }, [deviceId]);
  useEffect(() => load(), [load]);
  const change = async (next: boolean) => {
    // One save at a time, so an earlier answer cannot overwrite a later choice.
    if (saving.current) return;
    saving.current = true;
    const previous = !next;
    // The switch moves at once (§11); a save over a relay can take a while.
    setEnabled(next);
    setBusy(true);
    setSaved(false);
    setError(null);
    try {
      setEnabled((await updateContextInspectionSettings({ enabled: next }, deviceId)).enabled);
      setSaved(true);
    } catch (cause) {
      setEnabled(previous);
      setError({ message: cause instanceof Error ? cause.message : String(cause), retry: () => void change(next) });
    } finally {
      saving.current = false;
      setBusy(false);
    }
  };
  return (
    <div className="pm-res-card">
      <div className="pm-res-row" data-testid="context-inspection-setting" data-off={!enabled || undefined}>
        <label className="pm-res-label">
          <input type="checkbox" role="switch" checked={enabled ?? false} aria-busy={busy || undefined} disabled={enabled === null || unsupported}
            onChange={event => void change(event.target.checked)} />
          <span>{translate('settings.contextInspection')}</span>
        </label>
        <span className="pm-res-value">
          {saved && !error ? <span role="status" className="pm-res-saved">{translate('settings.resourcesSaved')}</span> : null}
          {unsupported ? <ErrorMark kind="unavailable" announce={false} message={translate('settings.contextInspectionUnavailable')} /> : null}
          {error ? <ErrorMark message={error.message} onRetry={error.retry} /> : null}
        </span>
        <p className="pm-res-hint">{translate('settings.contextInspectionHint')}</p>
      </div>
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
