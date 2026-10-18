import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { formatFileSize } from '@pockymoe/thread-ui';
import { HardDrive, RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { StorageScanDto, StorageSettingsDto, StorageThreadDto } from '@pockymoe/shared';
import {
  ApiError,
  compactStorage,
  fetchStorageOverview,
  fetchStorageSettings,
  pruneStorage,
  scanStorage,
  updateStorageSettings,
} from '../lib/api';
import { ConfirmDialog } from './ConfirmDialog';

const DEFAULT_DAYS = '30';

function percent(part: number, whole: number) {
  return whole > 0 ? Math.round((part / whole) * 100) : 0;
}

function parseDays(on: boolean, text: string): number | null | undefined {
  if (!on) return null;
  const days = Number(text);
  return Number.isInteger(days) && days >= 1 && days <= 3650 ? days : undefined;
}

/** Disk use of this device's Pockymoe database. It is measured only on request. */
export function StorageSettings({ deviceId }: { deviceId: string | null }) {
  const { locale } = useI18n();
  const scope = useId();
  const [deviceName, setDeviceName] = useState('');
  const [scan, setScan] = useState<StorageScanDto | null>(null);
  const [rows, setRows] = useState<StorageThreadDto[]>([]);
  const [unsupported, setUnsupported] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<StorageSettingsDto | null>(null);
  const [retention, setRetention] = useState({ on: false, text: DEFAULT_DAYS });
  const [savingRetention, setSavingRetention] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<'clean' | 'compact' | null>(null);
  const [working, setWorking] = useState(false);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  const sentinel = useRef<HTMLDivElement>(null);

  const apply = useCallback((next: StorageScanDto | null) => {
    setScan(next);
    setRows(next?.threads ?? []);
    if (next?.deviceName) setDeviceName(next.deviceName);
  }, []);

  useEffect(() => {
    let alive = true;
    Promise.all([fetchStorageOverview(0, deviceId), fetchStorageSettings(deviceId)]).then(([overview, settings]) => {
      if (!alive) return;
      setDeviceName(overview.deviceName);
      apply(overview.scan);
      setSaved(settings);
      setRetention({ on: settings.commandOutputRetentionDays != null, text: String(settings.commandOutputRetentionDays ?? DEFAULT_DAYS) });
    }).catch((cause: unknown) => {
      if (!alive) return;
      if (cause instanceof ApiError && cause.statusCode === 404) setUnsupported(true);
      else setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { alive = false; };
  }, [deviceId, apply]);

  const runScan = async () => {
    setScanning(true);
    setError(null);
    try {
      apply(await scanStorage(deviceId));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setScanning(false);
    }
  };

  const loadMore = useCallback(async () => {
    if (!scan?.nextOffset || loadingMore) return;
    setLoadingMore(true);
    try {
      const overview = await fetchStorageOverview(scan.nextOffset, deviceId);
      // A newer scan (or none after a cleanup) replaces the list instead of extending it.
      if (!overview.scan || overview.scan.scannedAt !== scan.scannedAt) apply(overview.scan);
      else {
        setScan(overview.scan);
        setRows(current => [...current, ...overview.scan!.threads]);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingMore(false);
    }
  }, [scan, loadingMore, deviceId, apply]);

  // Scrolling to the end of the list reads the next page.
  useEffect(() => {
    const node = sentinel.current;
    if (!node || !scan?.nextOffset || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) void loadMore();
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [scan?.nextOffset, loadMore]);

  const days = parseDays(retention.on, retention.text);
  const retentionChanged = saved != null && days !== undefined && days !== saved.commandOutputRetentionDays;
  const saveRetention = async () => {
    if (days === undefined || savingRetention) return;
    setSavingRetention(true);
    setNotice(null);
    setError(null);
    try {
      const next = await updateStorageSettings({ commandOutputRetentionDays: days }, deviceId);
      setSaved(next);
      setNotice(translate('settings.resourcesSaved'));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSavingRetention(false);
    }
  };

  const confirmAction = async () => {
    setWorking(true);
    setConfirmError(null);
    try {
      if (confirm === 'clean') {
        const result = await pruneStorage(deviceId);
        setNotice(result.items
          ? translate('settings.storageCleaned', { count: result.items, size: formatFileSize(result.bytes) })
          : translate('settings.storageNothingToClean'));
      } else {
        const result = await compactStorage(deviceId);
        setNotice(translate('settings.storageCompacted', { before: formatFileSize(result.beforeBytes), after: formatFileSize(result.afterBytes) }));
      }
      setConfirm(null);
      // Both change what the last scan measured; measure again.
      if (scan) await runScan();
    } catch (cause) {
      setConfirmError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setWorking(false);
    }
  };

  if (unsupported) return <p className="pm-res-empty">{translate('settings.storageUnavailable')}</p>;

  const scannedAt = scan ? new Date(scan.scannedAt).toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' }) : null;
  const largest = rows[0]?.bytes ?? 0;
  const savedDays = saved?.commandOutputRetentionDays ?? null;
  return (
    <>
      <div className="pm-res-card">
        <div className="pm-res-device">
          <HardDrive aria-hidden="true" />
          <span className="pm-res-device-text">
            <strong>{deviceName || '—'}</strong>
            <span>{scannedAt ? translate('settings.storageScannedAt', { time: scannedAt }) : translate('settings.storageScanHint')}</span>
          </span>
          <button type="button" className="host-secondary-button pm-res-button" disabled={scanning} onClick={() => void runScan()}>
            <RefreshCw aria-hidden="true" className={scanning ? 'is-spinning' : undefined} />
            {scanning ? translate('settings.storageScanning') : scan ? translate('settings.storageRescan') : translate('settings.storageScan')}
          </button>
        </div>
        {scan ? (
          <>
            <dl className="pm-res-stats">
              <div>
                <dt>{translate('settings.storageDatabase')}</dt>
                <dd>{formatFileSize(scan.databaseBytes)}</dd>
                <dd className="pm-res-stat-note">{translate('settings.storageFree')} {formatFileSize(scan.freeBytes)}</dd>
              </div>
              <div>
                <dt>{translate('settings.storageThreads')}</dt>
                <dd>{scan.threadCount.toLocaleString()}</dd>
                <dd className="pm-res-stat-note">{translate('settings.storageThreadCount', { conversations: scan.conversationCount.toLocaleString() })}</dd>
              </div>
              <div>
                <dt>{translate('settings.storageCommands')}</dt>
                <dd>{formatFileSize(scan.commandBytes)}</dd>
                <dd className="pm-res-stat-note">{translate('settings.storageCommandShare', { percent: percent(scan.commandBytes, scan.historyBytes) })}</dd>
              </div>
            </dl>
            <h4 className="pm-res-subheading">{translate('settings.storageLargest')}</h4>
            <ol className="pm-res-threads">
              {rows.map(thread => {
                const share = percent(thread.commandBytes, thread.bytes);
                return (
                  <li key={thread.threadId} className="pm-res-thread">
                    <span className="pm-res-thread-title" title={thread.title}>{thread.title || translate('settings.storageUntitled')}</span>
                    <span className="pm-res-thread-size">{formatFileSize(thread.bytes)}</span>
                    <span className="pm-res-thread-meta">
                      {[thread.workspaceLabel, thread.agentThreads ? translate('settings.storageAgentThreads', { count: thread.agentThreads }) : null].filter(Boolean).join(' · ')}
                    </span>
                    <span className="pm-res-thread-command">{translate('settings.storageCommandPart', { size: formatFileSize(thread.commandBytes), percent: share })}</span>
                    {/* Width shows the conversation against the largest; the fill is its command part. */}
                    <span className="pm-res-bar" aria-hidden="true" style={{ width: `${Math.max(2, percent(thread.bytes, largest))}%` }}>
                      <span style={{ width: `${share}%` }} />
                    </span>
                  </li>
                );
              })}
            </ol>
            {scan.nextOffset != null && (
              <div ref={sentinel} className="pm-res-more">
                <button type="button" className="host-secondary-button pm-res-button" disabled={loadingMore} onClick={() => void loadMore()}>
                  {loadingMore ? translate('settings.storageLoadingMore') : translate('settings.storageLoadMore')}
                </button>
              </div>
            )}
          </>
        ) : null}
      </div>

      <div className="pm-res-card">
        <div className="pm-res-row" data-off={!retention.on || undefined}>
          <label className="pm-res-label">
            <input type="checkbox" checked={retention.on} disabled={savingRetention}
              onChange={event => { setRetention(current => ({ ...current, on: event.target.checked })); setNotice(null); }} />
            <span>{translate('settings.storageRetention')}</span>
          </label>
          <span className="pm-res-value">
            <input id={`${scope}-days`} type="number" inputMode="numeric" min={1} max={3650} step={1}
              aria-label={translate('settings.storageRetention')} aria-invalid={days === undefined || undefined}
              aria-describedby={`${scope}-days-hint`} disabled={!retention.on || savingRetention} value={retention.text}
              onChange={event => { setRetention(current => ({ ...current, text: event.target.value })); setNotice(null); }}
              className="relay-input pm-res-input" />
            <span className="pm-res-unit">{translate('settings.storageRetentionUnit')}</span>
          </span>
          <p id={`${scope}-days-hint`} className={`pm-res-hint${days === undefined ? ' is-error' : ''}`}>
            {translate(days === undefined ? 'settings.storageRetentionRange' : 'settings.storageRetentionHint')}
            {scan && scan.prunedBytes > 0 ? ` ${translate('settings.storagePrunedTotal', { size: formatFileSize(scan.prunedBytes) })}` : ''}
          </p>
        </div>
        <div className="pm-res-row">
          <span className="pm-res-label is-plain">{translate('settings.storageCompact')}</span>
          <span className="pm-res-value is-action">
            <button type="button" className="ui-action-danger pm-res-button" onClick={() => { setConfirmError(null); setConfirm('compact'); }}>
              {translate('settings.storageCompactAction')}
            </button>
          </span>
          <p className="pm-res-hint">{translate('settings.storageCompactHint')}</p>
        </div>
        <div className="pm-res-footer">
          {notice && <span role="status" className="pm-res-saved">{notice}</span>}
          {error && <p className="host-error pm-res-error" role="alert">{error}</p>}
          <button type="button" className="ui-action-danger pm-res-button" disabled={savedDays == null || retentionChanged}
            onClick={() => { setConfirmError(null); setConfirm('clean'); }}>
            {translate('settings.storageCleanNow')}
          </button>
          <button type="button" className="relay-button-primary pm-res-primary" disabled={!retentionChanged || savingRetention} onClick={() => void saveRetention()}>
            {savingRetention ? translate('settings.saving') : translate('files.save')}
          </button>
        </div>
      </div>

      <ConfirmDialog
        open={confirm != null}
        title={translate(confirm === 'compact' ? 'settings.storageCompactTitle' : 'settings.storageCleanTitle')}
        description={confirm === 'compact'
          ? translate('settings.storageCompactBody', { size: formatFileSize(scan?.databaseBytes ?? 0) })
          : translate('settings.storageCleanBody', { days: savedDays ?? 0 })}
        confirmLabel={translate(confirm === 'compact' ? 'settings.storageCompact' : 'settings.storageCleanNow')}
        busyLabel={translate(confirm === 'compact' ? 'settings.storageCompacting' : 'settings.storageCleaning')}
        busy={working}
        error={confirmError}
        onCancel={() => { if (!working) setConfirm(null); }}
        onConfirm={() => void confirmAction()}
      />
    </>
  );
}
