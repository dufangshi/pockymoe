import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark, formatFileSize, useLongPress, useMenuKeyboard } from '@pockymoe/thread-ui';
import { Eraser, FileDiff, HardDrive, MoreHorizontal, RefreshCw, Scissors, SquareTerminal } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { CommandTextCleanup, StorageCleanupDto, StorageJobDto, StorageScanDto, StorageSettingsDto, StorageThreadDto } from '@pockymoe/shared';
import {
  ApiError,
  cleanStorageThread,
  compactStorage,
  fetchStorageOverview,
  fetchStorageSettings,
  pruneStorage,
  scanStorage,
  updateStorageSettings,
} from '../lib/api';
import { ConfirmDialog } from './ConfirmDialog';

const DEFAULT_DAYS = '30';
const DEFAULT_CHARS = 512;

function percent(part: number, whole: number) {
  return whole > 0 ? Math.round((part / whole) * 100) : 0;
}

function parseDays(on: boolean, text: string): number | null | undefined {
  if (!on) return null;
  const days = Number(text);
  return Number.isInteger(days) && days >= 1 && days <= 3650 ? days : undefined;
}

function parseChars(text: string): number | undefined {
  const chars = Number(text);
  return Number.isInteger(chars) && chars >= 16 && chars <= 10_000 ? chars : undefined;
}

/** What a cleanup removes, as a phrase for its confirmation. */
function describe(options: StorageCleanupDto) {
  return [
    options.commandOutput ? translate('settings.storageWhatOutput') : null,
    options.commandText === 'truncate' ? translate('settings.storageWhatTruncate', { chars: options.commandTextChars }) : null,
    options.commandText === 'remove' ? translate('settings.storageWhatRemove') : null,
    options.fileDiffs ? translate('settings.storageWhatDiffs') : null,
  ].filter(Boolean).join(translate('settings.storageWhatJoin'));
}

type ThreadAction = 'output' | 'diffs' | 'truncate' | 'remove';
type Pending = { kind: 'clean' } | { kind: 'compact' } | { kind: 'thread'; thread: StorageThreadDto; options: StorageCleanupDto };
/** A failure, shown as a mark after the control that started it. */
type Failure = { message: string; retry?: () => void };
const reason = (cause: unknown) => cause instanceof Error ? cause.message : String(cause);

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
  // Reading or measuring (mark by Scan), the next page (by Load more), saving
  // (by Save) and a job the device ran (by the control that started it).
  const [readError, setReadError] = useState<Failure | null>(null);
  const [moreError, setMoreError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [jobError, setJobError] = useState<{ message: string; job: StorageJobDto } | null>(null);
  const [revision, setRevision] = useState(0);
  const [saved, setSaved] = useState<StorageSettingsDto | null>(null);
  const [retention, setRetention] = useState({ on: false, text: DEFAULT_DAYS });
  const [output, setOutput] = useState(true);
  const [textMode, setTextMode] = useState<CommandTextCleanup>('keep');
  const [charsText, setCharsText] = useState(String(DEFAULT_CHARS));
  const [diffs, setDiffs] = useState(false);
  const [savingRetention, setSavingRetention] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Pending | null>(null);
  const [starting, setStarting] = useState(false);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  // Cleanup and compaction run on the device; this follows the job.
  const [job, setJob] = useState<StorageJobDto | null>(null);
  const sentinel = useRef<HTMLDivElement>(null);

  const apply = useCallback((next: StorageScanDto | null) => {
    setScan(next);
    setRows(next?.threads ?? []);
    if (next?.deviceName) setDeviceName(next.deviceName);
  }, []);

  const showSettings = useCallback((settings: StorageSettingsDto) => {
    setSaved(settings);
    setRetention({ on: settings.retentionDays != null, text: String(settings.retentionDays ?? DEFAULT_DAYS) });
    setOutput(settings.commandOutput);
    setTextMode(settings.commandText);
    setCharsText(String(settings.commandTextChars));
    setDiffs(settings.fileDiffs);
  }, []);

  useEffect(() => {
    let alive = true;
    setReadError(null);
    Promise.all([fetchStorageOverview(0, deviceId), fetchStorageSettings(deviceId)]).then(([overview, settings]) => {
      if (!alive) return;
      setDeviceName(overview.deviceName);
      apply(overview.scan);
      setJob(overview.job ?? null);
      showSettings(settings);
    }).catch((cause: unknown) => {
      if (!alive) return;
      if (cause instanceof ApiError && cause.statusCode === 404) setUnsupported(true);
      else setReadError({ message: reason(cause), retry: () => setRevision(value => value + 1) });
    });
    return () => { alive = false; };
  }, [deviceId, apply, showSettings, revision]);

  // `quiet`: the measurement after a job, on its own; its failure keeps the last one.
  const runScan = useCallback(async (quiet = false) => {
    setScanning(true);
    if (!quiet) setReadError(null);
    try {
      apply(await scanStorage(deviceId));
      setReadError(null);
    } catch (cause) {
      if (!quiet) setReadError({ message: reason(cause), retry: () => void runScan() });
    } finally {
      setScanning(false);
    }
  }, [deviceId, apply]);

  // Follow a running job every second; when it ends, say what it did and measure again.
  const running = job?.state === 'running';
  const hadScan = scan != null;
  useEffect(() => {
    if (!running) return;
    let alive = true;
    const timer = window.setInterval(() => {
      void fetchStorageOverview(0, deviceId).then(overview => {
        if (!alive) return;
        const next = overview.job ?? null;
        setJob(next);
        if (next?.state === 'done') {
          setNotice(next.action === 'compact'
            ? translate('settings.storageCompacted', { before: formatFileSize(next.beforeBytes ?? 0), after: formatFileSize(next.afterBytes ?? 0) })
            : next.items
              ? translate('settings.storageCleaned', { count: next.items.toLocaleString(), size: formatFileSize(next.bytes) })
              : translate('settings.storageNothingToClean'));
          if (hadScan) void runScan(true);
        } else if (next?.state === 'failed') {
          setJobError({ message: translate('settings.storageJobFailed', { reason: next.error ?? '' }), job: next });
        }
      }).catch(() => { /* The device may pause during compaction; keep following. */ });
    }, 1000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [running, deviceId, hadScan, runScan]);

  const loadMore = useCallback(async () => {
    if (!scan?.nextOffset || loadingMore) return;
    setLoadingMore(true);
    setMoreError(null);
    try {
      const overview = await fetchStorageOverview(scan.nextOffset, deviceId);
      // A newer scan (or none after a cleanup) replaces the list instead of extending it.
      if (!overview.scan || overview.scan.scannedAt !== scan.scannedAt) apply(overview.scan);
      else {
        setScan(overview.scan);
        setRows(current => [...current, ...overview.scan!.threads]);
      }
    } catch (cause) {
      setMoreError(reason(cause));
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
  const chars = parseChars(charsText);
  const draft: StorageCleanupDto | undefined = chars === undefined && textMode === 'truncate' ? undefined : {
    commandOutput: output,
    commandText: textMode,
    commandTextChars: chars ?? saved?.commandTextChars ?? DEFAULT_CHARS,
    fileDiffs: diffs,
  };
  const nothingChosen = retention.on && !output && textMode === 'keep' && !diffs;
  const valid = days !== undefined && draft !== undefined && !nothingChosen;
  const changed = saved != null && valid && (days !== saved.retentionDays
    || draft.commandOutput !== saved.commandOutput || draft.commandText !== saved.commandText
    || draft.commandTextChars !== saved.commandTextChars || draft.fileDiffs !== saved.fileDiffs);
  const saveRetention = async () => {
    if (!valid || savingRetention) return;
    setSavingRetention(true);
    setNotice(null);
    setSaveError(null);
    try {
      const next = await updateStorageSettings({ retentionDays: days, ...draft }, deviceId);
      showSettings(next);
      setNotice(translate('settings.resourcesSaved'));
      // A saved age starts a cleanup on the device.
      if (next.retentionDays != null) setJob((await fetchStorageOverview(0, deviceId)).job ?? null);
    } catch (cause) {
      setSaveError(reason(cause));
    } finally {
      setSavingRetention(false);
    }
  };
  const edited = () => { setNotice(null); setSaveError(null); };

  // The device answers at once; the job then runs there with its progress below.
  const confirmAction = async () => {
    if (!confirm) return;
    setStarting(true);
    setConfirmError(null);
    try {
      setJob(confirm.kind === 'clean' ? await pruneStorage(deviceId)
        : confirm.kind === 'compact' ? await compactStorage(deviceId)
          : await cleanStorageThread(confirm.thread.threadId, confirm.options, deviceId));
      setNotice(null);
      setJobError(null);
      setConfirm(null);
    } catch (cause) {
      setConfirmError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setStarting(false);
    }
  };

  const savedChars = saved?.commandTextChars ?? DEFAULT_CHARS;
  const cleanThread = (thread: StorageThreadDto, action: ThreadAction) => {
    const options: StorageCleanupDto = {
      commandOutput: action === 'output',
      commandText: action === 'truncate' ? 'truncate' : action === 'remove' ? 'remove' : 'keep',
      commandTextChars: savedChars,
      fileDiffs: action === 'diffs',
    };
    setConfirmError(null);
    setConfirm({ kind: 'thread', thread, options });
  };

  if (unsupported) return <p className="pm-res-empty">{translate('settings.storageUnavailable')}</p>;

  const scannedAt = scan ? new Date(scan.scannedAt).toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' }) : null;
  const largest = rows[0]?.bytes ?? 0;
  const savedDays = saved?.retentionDays ?? null;
  const counted = running && job.action !== 'compact' && job.total > 0;
  const progress = counted ? Math.min(100, (job.processed / job.total) * 100) : null;
  const jobText = !running ? null : job.action === 'compact'
    ? translate('settings.storageJobCompacting')
    : job.total === 0
      ? translate('settings.storageJobFinding')
      : job.action === 'dedupe'
        ? translate('settings.storageJobDedupe', { done: job.processed.toLocaleString(), total: job.total.toLocaleString() })
        : translate('settings.storageJobPruning', { done: job.processed.toLocaleString(), total: job.total.toLocaleString(), size: formatFileSize(job.bytes) });
  const confirmText = (() => {
    if (!confirm) return { title: '', body: '', label: '', busy: '' };
    if (confirm.kind === 'compact') {
      return {
        title: translate('settings.storageCompactTitle'),
        body: translate('settings.storageCompactBody', { size: formatFileSize(scan?.databaseBytes ?? 0) }),
        label: translate('settings.storageCompact'),
        busy: translate('settings.storageCompacting'),
      };
    }
    if (confirm.kind === 'clean') {
      return {
        title: translate('settings.storageCleanTitle'),
        body: saved ? translate('settings.storageCleanBody', { what: describe(saved), days: savedDays ?? 0 }) : '',
        label: translate('settings.storageCleanNow'),
        busy: translate('settings.storageCleaning'),
      };
    }
    const title = confirm.thread.title || translate('settings.storageUntitled');
    return {
      title: translate('settings.storageThreadTitle', { title }),
      body: translate('settings.storageThreadBody', { what: describe(confirm.options) }),
      label: translate('settings.storageCleanNow'),
      busy: translate('settings.storageCleaning'),
    };
  })();
  return (
    <>
      <div className="pm-res-card">
        <div className="pm-res-device">
          <HardDrive aria-hidden="true" />
          <span className="pm-res-device-text">
            <strong>{deviceName || '—'}</strong>
            <span>{scannedAt ? translate('settings.storageScannedAt', { time: scannedAt }) : translate('settings.storageScanHint')}</span>
          </span>
          <span className="inline-flex shrink-0 items-center gap-1">
            <button type="button" className="host-secondary-button pm-res-button" disabled={scanning} onClick={() => void runScan()}>
              <RefreshCw aria-hidden="true" className={scanning ? 'is-spinning' : undefined} />
              {scanning ? translate('settings.storageScanning') : scan ? translate('settings.storageRescan') : translate('settings.storageScan')}
            </button>
            {readError && !scanning && <ErrorMark message={readError.message} onRetry={readError.retry} />}
          </span>
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
              <div>
                <dt>{translate('settings.storageDiffs')}</dt>
                <dd>{formatFileSize(scan.fileChangeBytes ?? 0)}</dd>
                <dd className="pm-res-stat-note">{translate('settings.storageCommandShare', { percent: percent(scan.fileChangeBytes ?? 0, scan.historyBytes) })}</dd>
              </div>
            </dl>
            <h4 className="pm-res-subheading">{translate('settings.storageLargest')}</h4>
            <ol className="pm-res-threads">
              {rows.map(thread => (
                <ThreadRow key={thread.threadId} thread={thread} largest={largest} savedChars={savedChars}
                  job={job?.threadId === thread.threadId && running ? job : null} disabled={running} onClean={cleanThread}
                  error={jobError?.job.threadId === thread.threadId ? jobError.message : null} />
              ))}
            </ol>
            {scan.nextOffset != null && (
              <div ref={sentinel} className="pm-res-more items-center gap-1">
                <button type="button" className="host-secondary-button pm-res-button" disabled={loadingMore} onClick={() => void loadMore()}>
                  {loadingMore ? translate('settings.storageLoadingMore') : translate('settings.storageLoadMore')}
                </button>
                {moreError && !loadingMore && <ErrorMark message={moreError} onRetry={() => void loadMore()} />}
              </div>
            )}
          </>
        ) : null}
      </div>

      <div className="pm-res-card">
        <div className="pm-res-row" data-off={!retention.on || undefined}>
          <label className="pm-res-label">
            <input type="checkbox" checked={retention.on} disabled={savingRetention}
              onChange={event => { setRetention(current => ({ ...current, on: event.target.checked })); edited(); }} />
            <span>{translate('settings.storageRetention')}</span>
          </label>
          <span className="pm-res-value">
            <input id={`${scope}-days`} type="number" inputMode="numeric" min={1} max={3650} step={1}
              aria-label={translate('settings.storageRetention')} aria-invalid={days === undefined || undefined}
              aria-describedby={`${scope}-days-hint`} disabled={!retention.on || savingRetention} value={retention.text}
              onChange={event => { setRetention(current => ({ ...current, text: event.target.value })); edited(); }}
              className="relay-input pm-res-input" />
            <span className="pm-res-unit">{translate('settings.storageRetentionUnit')}</span>
          </span>
          <p id={`${scope}-days-hint`} className={`pm-res-hint${days === undefined || nothingChosen ? ' is-error' : ''}`}>
            {translate(days === undefined ? 'settings.storageRetentionRange' : nothingChosen ? 'settings.storageCleanNothing' : 'settings.storageRetentionHint')}
            {scan && scan.prunedBytes > 0 ? ` ${translate('settings.storagePrunedTotal', { size: formatFileSize(scan.prunedBytes) })}` : ''}
          </p>
        </div>
        {/* What a cleanup removes: the automatic one and "Clean up now" alike. */}
        <div className="pm-res-options" data-off={!retention.on || undefined}>
          <div className="pm-res-row">
            <label className="pm-res-label">
              <input type="checkbox" checked={output} disabled={savingRetention} onChange={event => { setOutput(event.target.checked); edited(); }} />
              <span>{translate('settings.storageCleanOutput')}</span>
            </label>
            <p className="pm-res-hint">{translate('settings.storageCleanOutputHint')}</p>
          </div>
          <div className="pm-res-row">
            <label className="pm-res-label is-plain" htmlFor={`${scope}-text`}>{translate('settings.storageCleanText')}</label>
            <span className="pm-res-value is-action">
              <select id={`${scope}-text`} className="relay-input pm-res-select" value={textMode} disabled={savingRetention}
                onChange={event => { setTextMode(event.target.value as CommandTextCleanup); edited(); }}>
                <option value="keep">{translate('settings.storageCleanTextKeep')}</option>
                <option value="truncate">{translate('settings.storageCleanTextTruncate')}</option>
                <option value="remove">{translate('settings.storageCleanTextRemove')}</option>
              </select>
            </span>
            <p className="pm-res-hint">{translate('settings.storageCleanTextHint')}</p>
          </div>
          {textMode === 'truncate' && (
            <div className="pm-res-row">
              <label className="pm-res-label is-plain" htmlFor={`${scope}-chars`}>{translate('settings.storageCleanChars')}</label>
              <span className="pm-res-value">
                <input id={`${scope}-chars`} type="number" inputMode="numeric" min={16} max={10000} step={1}
                  aria-invalid={chars === undefined || undefined} disabled={savingRetention} value={charsText}
                  onChange={event => { setCharsText(event.target.value); edited(); }} className="relay-input pm-res-input" />
                <span className="pm-res-unit">{translate('settings.storageCleanCharsUnit')}</span>
              </span>
              {chars === undefined && <p className="pm-res-hint is-error">{translate('settings.storageCleanCharsRange')}</p>}
            </div>
          )}
          <div className="pm-res-row">
            <label className="pm-res-label">
              <input type="checkbox" checked={diffs} disabled={savingRetention} onChange={event => { setDiffs(event.target.checked); edited(); }} />
              <span>{translate('settings.storageCleanDiffs')}</span>
            </label>
            <p className="pm-res-hint">{translate('settings.storageCleanDiffsHint')}</p>
          </div>
        </div>
        <div className="pm-res-row">
          <span className="pm-res-label is-plain">{translate('settings.storageCompact')}</span>
          <span className="pm-res-value is-action">
            <span className="inline-flex items-center gap-1">
              <button type="button" className="ui-action-danger pm-res-button" disabled={running} onClick={() => { setConfirmError(null); setConfirm({ kind: 'compact' }); }}>
                {translate('settings.storageCompactAction')}
              </button>
              {jobError?.job.action === 'compact' && <ErrorMark message={jobError.message} />}
            </span>
          </span>
          <p className="pm-res-hint">{translate('settings.storageCompactHint')}</p>
        </div>
        {jobText && !job?.threadId && (
          <div className="pm-res-job" role="status">
            <span className="pm-res-job-text">{jobText}</span>
            <span className={`pm-res-job-bar${progress == null ? ' is-indeterminate' : ''}`} aria-hidden="true">
              <span style={progress == null ? undefined : { width: `${progress}%` }} />
            </span>
          </div>
        )}
        <div className="pm-res-footer">
          {notice && <span role="status" className="pm-res-saved">{notice}</span>}
          <button type="button" className="ui-action-danger pm-res-button" disabled={savedDays == null || changed || running}
            onClick={() => { setConfirmError(null); setConfirm({ kind: 'clean' }); }}>
            {translate('settings.storageCleanNow')}
          </button>
          {/* A cleanup the device ran by age (or its dedupe pass) failed. */}
          {jobError && jobError.job.action !== 'compact' && !jobError.job.threadId && <ErrorMark message={jobError.message} />}
          <button type="button" className="relay-button-primary pm-res-primary" disabled={!changed || savingRetention || running} onClick={() => void saveRetention()}>
            {savingRetention ? translate('settings.saving') : translate('files.save')}
          </button>
          {saveError && !savingRetention && <ErrorMark message={saveError} onRetry={() => void saveRetention()} />}
        </div>
      </div>

      <ConfirmDialog
        open={confirm != null}
        title={confirmText.title}
        description={confirmText.body}
        confirmLabel={confirmText.label}
        busyLabel={confirmText.busy}
        busy={starting}
        error={confirmError}
        onCancel={() => { if (!starting) setConfirm(null); }}
        onConfirm={() => void confirmAction()}
      />
    </>
  );
}

/** A scanned conversation with its cleanup menu: ⋯, right click or long press. */
function ThreadRow({ thread, largest, savedChars, job, disabled, onClean, error }: {
  thread: StorageThreadDto;
  largest: number;
  savedChars: number;
  job: StorageJobDto | null;
  disabled: boolean;
  onClean: (thread: StorageThreadDto, action: ThreadAction) => void;
  /** Why this conversation's last cleanup failed. */
  error: string | null;
}) {
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const title = thread.title || translate('settings.storageUntitled');
  const openAt = useCallback((x: number, y: number) => {
    setPosition({ left: Math.min(Math.max(8, x - 240), innerWidth - 248), top: Math.max(8, Math.min(y + 6, innerHeight - 220)) });
  }, []);
  const longPress = useLongPress(point => openAt(point.x, point.y));
  useMenuKeyboard(menu, Boolean(position), restore => { setPosition(null); if (restore) trigger.current?.focus(); });
  useEffect(() => {
    if (!position) return;
    const outside = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) setPosition(null);
    };
    // The menu belongs to its row: scrolling the list away closes it.
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) setPosition(null); };
    const resize = () => setPosition(null);
    document.addEventListener('pointerdown', outside);
    window.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', resize);
    return () => {
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', resize);
    };
  }, [position]);
  const choose = (action: ThreadAction) => { setPosition(null); onClean(thread, action); };
  const item = (action: ThreadAction, icon: ReactNode, label: string, empty = false) => (
    <button type="button" role="menuitem" disabled={disabled || empty} onClick={() => choose(action)}>{icon}{label}</button>
  );
  const commandShare = percent(thread.commandBytes, thread.bytes);
  const diffShare = percent(thread.fileChangeBytes ?? 0, thread.bytes);
  const progress = job && job.total > 0 ? Math.min(100, (job.processed / job.total) * 100) : null;
  return (
    <li className="pm-res-thread" data-cleaning={job ? true : undefined}
      onContextMenu={event => { event.preventDefault(); openAt(event.clientX, event.clientY); }} {...longPress}>
      {error ? (
        // The mark stays whole beside a title that shortens.
        <span className="flex min-w-0 items-center gap-1">
          <span className="pm-res-thread-title" title={thread.title}>{title}</span>
          <ErrorMark message={error} />
        </span>
      ) : <span className="pm-res-thread-title" title={thread.title}>{title}</span>}
      <span className="pm-res-thread-size">{formatFileSize(thread.bytes)}</span>
      <button ref={trigger} type="button" className="pm-res-thread-actions" aria-label={translate('settings.storageThreadActions', { title })}
        aria-haspopup="menu" aria-expanded={Boolean(position)} onClick={() => {
          if (position) { setPosition(null); return; }
          const box = trigger.current!.getBoundingClientRect();
          openAt(box.right, box.bottom);
        }}>
        <MoreHorizontal aria-hidden="true" size={16} />
      </button>
      <span className="pm-res-thread-meta">
        {[thread.workspaceLabel, thread.agentThreads ? translate('settings.storageAgentThreads', { count: thread.agentThreads }) : null].filter(Boolean).join(' · ')}
      </span>
      <span className="pm-res-thread-command">
        {job
          ? translate('settings.storageThreadCleaning', { done: job.processed.toLocaleString(), total: job.total.toLocaleString() })
          : translate('settings.storageThreadParts', { commands: formatFileSize(thread.commandBytes), diffs: formatFileSize(thread.fileChangeBytes ?? 0) })}
      </span>
      {job ? (
        <span className={`pm-res-job-bar pm-res-thread-job${progress == null ? ' is-indeterminate' : ''}`} role="progressbar"
          aria-label={translate('settings.storageCleaning')} aria-valuenow={progress == null ? undefined : Math.round(progress)}>
          <span style={progress == null ? undefined : { width: `${progress}%` }} />
        </span>
      ) : (
        // Width shows the conversation against the largest; the fills are its commands and diffs.
        <span className="pm-res-bar" aria-hidden="true" style={{ width: `${Math.max(2, percent(thread.bytes, largest))}%` }}>
          <span style={{ width: `${commandShare}%` }} />
          <span className="is-diff" style={{ width: `${diffShare}%` }} />
        </span>
      )}
      {/* Inside the settings window, which keeps focus within itself; it has no
          transform, so the fixed position still follows the viewport. */}
      {position && createPortal(
        <div ref={menu} role="menu" aria-label={translate('settings.storageThreadActions', { title })} className="pm-res-menu" style={position}>
          {item('output', <SquareTerminal aria-hidden="true" size={15} />, translate('settings.storageThreadOutput'), thread.commandBytes === 0)}
          {item('diffs', <FileDiff aria-hidden="true" size={15} />, translate('settings.storageThreadDiffs'), !thread.fileChangeBytes)}
          {item('truncate', <Scissors aria-hidden="true" size={15} />, translate('settings.storageThreadTruncate', { chars: savedChars }), thread.commandBytes === 0)}
          {item('remove', <Eraser aria-hidden="true" size={15} />, translate('settings.storageThreadRemove'), thread.commandBytes === 0)}
        </div>,
        trigger.current?.closest<HTMLElement>('[role="dialog"]') ?? document.body,
      )}
    </li>
  );
}
