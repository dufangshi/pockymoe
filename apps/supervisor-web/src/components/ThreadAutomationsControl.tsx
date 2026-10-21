import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  Workflow,
  RefreshCw,
  Clock3,
  ChevronRight,
  Info,
  MessageSquare,
  Inbox,
  Terminal,
  Puzzle,
  CircleSlash,
} from 'lucide-react';
import './automation-panel.css';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  ErrorMark,
  TokenUsageCost,
  PluginActionOutput,
  type TokenUsageCostProps,
} from '@pockymoe/thread-ui';
import { useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import type {
  AutomationDto,
  AutomationRunDto,
  AutomationTrigger,
  ThreadDto,
} from '@pockymoe/shared';
import { request } from '../lib/api';
import {
  NativeWatchCard,
  isCurrentWatch,
  type NativeWatch,
} from './ThreadWatchesControl';

const knownStates = new Set([
  'enabled',
  'paused',
  'cancelled',
  'due',
  'queued',
  'running',
  'completed',
  'failed',
  'interrupted',
  'timedOut',
  'uncertain',
  'conditionSkipped',
  'loopSkipped',
  'skipped',
]);
const currentAutomation = (a: AutomationDto) =>
  a.state === 'enabled' &&
  (a.definition.trigger.kind !== 'at' ||
    a.nextRunAt != null ||
    a.pendingCount > 0 ||
    (a.statistics?.runningActionCount ?? 0) > 0);

/** Add only attributed known values; coverage remains explicit alongside the sum. */
export function automationTotals(
  items: AutomationDto[],
  watches: NativeWatch[],
) {
  const entries = [
    ...items.map((a) => ({
      count: a.statistics?.triggerCount,
      usage: a.definition.action.kind === 'pluginAction' ? null : a.statistics?.tokenUsage,
      price: a.definition.action.kind === 'pluginAction' ? null : a.statistics?.priceEstimate,
    })),
    ...watches.map((w) => ({
      count: w.triggerCount,
      usage: w.tokenUsage,
      price: w.priceEstimate,
    })),
  ];
  const usage = {
    totalTokens: 0,
    inputTokens: 0,
    cachedInputTokens: 0,
    cacheWriteInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
  };
  const price = {
    totalUsd: 0,
    inputUsd: 0,
    cachedInputUsd: 0,
    cacheWriteInputUsd: 0,
    outputUsd: 0,
  };
  let triggers = 0,
    unknownCounts = 0,
    usageCount = 0,
    priceCount = 0;
  for (const entry of entries) {
    if (entry.count == null) unknownCounts++;
    else triggers += entry.count;
    if (entry.usage) {
      usageCount++;
      for (const key of Object.keys(usage) as (keyof typeof usage)[])
        usage[key] += entry.usage[key] ?? 0;
    }
    if (entry.price) {
      priceCount++;
      for (const key of Object.keys(price) as (keyof typeof price)[])
        price[key] += entry.price[key] ?? 0;
    }
  }
  // Native ambiguous matches cannot be deduplicated across watch cards here.
  // Keep the confirmed sum and mark the thread trigger total incomplete.
  unknownCounts += Number(
    watches.some((w) => (w.ambiguousTriggerCount ?? 0) > 0),
  );
  return {
    triggers,
    unknownCounts,
    usage: usageCount || !entries.length ? usage : null,
    price: priceCount || !entries.length ? price : null,
  };
}

function Usage({
  usage,
  price,
  note,
}: {
  usage?: TokenUsageCostProps['usage'];
  price?: TokenUsageCostProps['price'];
  note?: string;
}) {
  const { t, locale } = useI18n();
  return (
    <span className="automation-rule-usage">
      <span>
        {usage
          ? t('automation.tokensValue', {
              value1: usage.totalTokens.toLocaleString(locale),
            })
          : t('automation.tokensUnknown')}
      </span>
      <span>
        {usage || price ? (
          <TokenUsageCost
            usage={usage ?? null}
            price={price ?? null}
            costLabel={t('automation.cost')}
            detailsNote={note ?? ''}
            tooltipZIndex={120}
          />
        ) : (
          t('automation.costUnknown')
        )}
      </span>
    </span>
  );
}

/** Short local time for scanning; the full timestamp stays in the tooltip and run details. */
function useDates() {
  const { t, locale } = useI18n();
  const full = (v: string | null) =>
    v ? new Date(v).toLocaleString(locale) : t('automation.none');
  const short = (v: string) => {
    const d = new Date(v);
    if (Number.isNaN(d.getTime())) return v;
    const now = new Date();
    const options: Intl.DateTimeFormatOptions =
      d.toDateString() === now.toDateString()
        ? { hour: '2-digit', minute: '2-digit' }
        : d.getFullYear() === now.getFullYear()
          ? { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }
          : { year: 'numeric', month: 'short', day: 'numeric' };
    return new Intl.DateTimeFormat(locale, options).format(d);
  };
  const duration = (from: string | null, to: string | null) => {
    if (!from || !to) return null;
    const seconds = Math.round((Date.parse(to) - Date.parse(from)) / 1000);
    if (!Number.isFinite(seconds) || seconds < 0) return null;
    const [value, unit] =
      seconds < 60
        ? [seconds, 'second']
        : seconds < 3600
          ? [Math.round(seconds / 60), 'minute']
          : [Math.round(seconds / 360) / 10, 'hour'];
    return new Intl.NumberFormat(locale, {
      style: 'unit',
      unit,
      unitDisplay: 'narrow',
    }).format(value);
  };
  return { full, short, duration };
}

function Time({ value }: { value: string }) {
  const { full, short } = useDates();
  return (
    <time dateTime={value} title={full(value)}>
      {short(value)}
    </time>
  );
}

function RunHistory({
  automation: a,
  runs,
  threadId,
}: {
  automation: AutomationDto;
  runs: AutomationRunDto[];
  threadId: string;
}) {
  const { t, locale } = useI18n();
  const { full, duration } = useDates();
  const [output, setOutput] = useState('');
  // Why a run's Output could not be read: a mark after that button.
  const [outputError, setOutputError] = useState<{ runId: string; message: string } | null>(null);
  const state = (v: string) =>
    knownStates.has(v) ? t(`automation.${v}` as TranslationKey) : v;
  const action = a.definition.action;
  return (
    <>
      {!runs.length && (
        <p className="automation-runs-empty">{t('automation.noRuns')}</p>
      )}
      {runs.length > 0 && (
        <ol className="automation-runs">
          {runs.map((r, index) => {
            const at = r.completedAt ?? r.startedAt ?? r.scheduledAt;
            const took = duration(r.startedAt, r.completedAt);
            return (
              <li key={r.id}>
                <details className="automation-run" open={index === 0}>
                  <summary>
                    <ChevronRight size={12} className="automation-run-chevron" />
                    <span className={`automation-state is-${r.state}`}>
                      <i />
                      {state(r.state)}
                    </span>
                    <span className="automation-run-when">
                      <Time value={at} />
                      {took && <span>· {took}</span>}
                      {r.missedCount > 0 && (
                        <span>
                          · {t('automation.mergedShort', { value1: r.missedCount })}
                        </span>
                      )}
                    </span>
                    {/* A run's error is the record of what happened, not a failure of
                        this window: it stays text. As marks, every failed run in
                        the history would be announced as an alert. */}
                    {r.error && (
                      <span className="automation-run-error-preview">
                        {r.error}
                      </span>
                    )}
                  </summary>
                  <div className="automation-run-body">
                    {r.error && (
                      <p className="plugin-report-error" role="status">
                        {r.error}
                      </p>
                    )}
                    {r.deliveryReceipt &&
                      Object.prototype.hasOwnProperty.call(
                        r.deliveryReceipt,
                        'output',
                      ) && (
                        <PluginActionOutput
                          pluginId={typeof r.deliveryReceipt.pluginId === 'string' ? r.deliveryReceipt.pluginId : action.kind === 'pluginAction' ? action.pluginId : ''}
                          action={typeof r.deliveryReceipt.action === 'string' ? r.deliveryReceipt.action : action.kind === 'pluginAction' ? action.action : ''}
                          output={r.deliveryReceipt.output}
                        />
                      )}
                    <div className="automation-run-tools">
                      {r.commandId && (
                        <span className="inline-flex items-center gap-1">
                          <button
                            type="button"
                            className="automation-link-button"
                            onClick={() => {
                              setOutputError(null);
                              void request<Record<string, unknown>>(
                                `api/threads/${encodeURIComponent(threadId)}/commands/${encodeURIComponent(r.commandId!)}`,
                              )
                                .then((v) =>
                                  setOutput(
                                    [String(v.stdout ?? ''), String(v.stderr ?? '')]
                                      .filter(Boolean)
                                      .join('\n') || t('automation.none'),
                                  ),
                                )
                                .catch((e) => setOutputError({ runId: r.id, message: String(e.message ?? e) }));
                            }}
                          >
                            {t('automation.output')}
                          </button>
                          {outputError?.runId === r.id && <ErrorMark message={outputError.message} />}
                        </span>
                      )}
                      <details className="plugin-report-disclosure">
                        <summary>{t('automation.runDetails')}</summary>
                        <dl className="automation-run-facts">
                          <dt>{t('automation.scheduled')}</dt>
                          <dd>{full(r.scheduledAt)}</dd>
                          <dt>{t('automation.started')}</dt>
                          <dd>{full(r.startedAt)}</dd>
                          <dt>{t('automation.finished')}</dt>
                          <dd>{full(r.completedAt)}</dd>
                          <dt>{t('automation.merged')}</dt>
                          <dd>{r.missedCount.toLocaleString(locale)}</dd>
                          {r.turnId && (
                            <>
                              <dt>{t('automation.turn')}</dt>
                              <dd><code>{r.turnId}</code></dd>
                            </>
                          )}
                        </dl>
                      </details>
                    </div>
                  </div>
                </details>
              </li>
            );
          })}
        </ol>
      )}
      {output && <pre className="plugin-report-pre">{output}</pre>}
    </>
  );
}

function AutomationCard({
  automation: a,
  threadId,
}: {
  automation: AutomationDto;
  threadId: string;
}) {
  const { t, locale } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [runs, setRuns] = useState<AutomationRunDto[]>([]);
  // The history refreshes every 3 s: a failed refresh keeps the last runs, and
  // only a history that never loaded says why.
  const [runsLoaded, setRunsLoaded] = useState(false);
  const [error, setError] = useState('');
  const retryRuns = useRef<() => void>(() => {});
  const detailsId = useId();
  const historyId = useId();
  const base = `api/threads/${encodeURIComponent(threadId)}/automations/${encodeURIComponent(a.id)}`;
  const { short } = useDates();
  const state = (v: string) =>
    knownStates.has(v) ? t(`automation.${v}` as TranslationKey) : v;
  const triggerLabel = (v: AutomationTrigger) =>
    v.kind === 'interval' && (v.everySeconds === 60 || v.everySeconds === 3600)
      ? t(v.everySeconds === 60 ? 'automation.everyMinute' : 'automation.everyHour')
      : v.kind === 'interval'
      ? t(
          v.everySeconds % 3600 === 0
            ? 'automation.hourly'
            : v.everySeconds % 60 === 0
              ? 'automation.minutely'
              : 'automation.intervalSummary',
          {
            value1:
              v.everySeconds % 3600 === 0
                ? v.everySeconds / 3600
                : v.everySeconds % 60 === 0
                  ? v.everySeconds / 60
                  : v.everySeconds,
          },
        )
      : v.kind === 'at'
        ? short(v.at)
        : v.kind === 'threadEnded'
          ? t('automation.threadEnded')
          : t(`automation.${v.kind}`);
  useEffect(() => {
    if (!expanded) return;
    const controller = new AbortController();
    let refreshing = false;
    const update = async () => {
      if (refreshing || document.visibilityState === 'hidden') return;
      refreshing = true;
      try {
        const data = await request<{ runs: AutomationRunDto[] }>(
          `${base}/runs`,
          { signal: controller.signal, cache: 'no-store' },
        );
        if (!controller.signal.aborted) {
          setRuns(data.runs);
          setRunsLoaded(true);
          setError('');
        }
      } catch (e) {
        if (!controller.signal.aborted)
          setError(String(e instanceof Error ? e.message : e));
      } finally {
        refreshing = false;
      }
    };
    retryRuns.current = () => void update();
    void update();
    const timer = window.setInterval(() => void update(), 3000);
    return () => {
      controller.abort();
      retryRuns.current = () => {};
      window.clearInterval(timer);
    };
  }, [expanded, base]);
  const s = a.statistics;
  const pluginAction = a.definition.action.kind === 'pluginAction';
  const coverage = pluginAction ? t('automation.pluginCharge') : s
    ? t('automation.coverage', {
        value1: s.usageTurnCount,
        value2: s.pricedTurnCount,
        value3: s.promptTurnCount,
      })
    : t('automation.unknownStatistics');
  return (
    <article className="automation-rule">
      <div className="automation-rule-heading">
        <strong className="break-words">{a.definition.name}</strong>
        <span className="flex shrink-0 items-center gap-1">
          <span className={`automation-state is-${a.state}`}>
            <i />
            {state(a.state)}
          </span>
          {/* The rule's last failure: a mark beside its state, the reason on a tap. */}
          {a.error && <ErrorMark message={a.error} />}
        </span>
      </div>
      <div className="automation-rule-schedule">
        <Clock3 size={12} />
        {triggerLabel(a.definition.trigger)}
        <span aria-hidden="true">·</span>
        {a.definition.action.kind === 'prompt' ? (
          <MessageSquare size={12} />
        ) : a.definition.action.kind === 'notifyInbox' ? (
          <Inbox size={12} />
        ) : pluginAction ? (
          <Puzzle size={12} />
        ) : (
          <Terminal size={12} />
        )}
        {t(`automation.${a.definition.action.kind}`)}
        {a.nextRunAt && (
          <>
            <span aria-hidden="true">·</span>
            <span>
              {t('automation.nextShort')} <Time value={a.nextRunAt} />
            </span>
          </>
        )}
      </div>
      <div className="automation-rule-meta">
        <span>
          {s
            ? t(s.triggerCount === 1 ? 'automation.triggerCountOne' : 'automation.triggerCount', {
                value1: s.triggerCount.toLocaleString(locale),
              })
            : `${t('automation.triggers')} —`}
        </span>
        {a.pendingCount > 0 && (
          <span>
            {t('automation.pendingShort', { value1: a.pendingCount })}
          </span>
        )}
        <Usage usage={pluginAction ? null : s?.tokenUsage} price={pluginAction ? null : s?.priceEstimate} note={coverage} />
      </div>
      <div className="automation-rule-actions">
        <button
          type="button"
          aria-expanded={detailsOpen}
          aria-controls={detailsId}
          onClick={() => setDetailsOpen(!detailsOpen)}
        >
          <ChevronRight size={12} />
          {t('automation.ruleDetails')}
        </button>
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={historyId}
          onClick={() => setExpanded(!expanded)}
        >
          <ChevronRight size={12} />
          {t('automation.history')}
        </button>
        {expanded && (
          <details className="automation-help">
            <summary
              aria-label={t('automation.historyHelp')}
              title={t('automation.historyHelp')}
            >
              <Info size={13} />
            </summary>
            <div>
              <p>{t('automation.historyLimit')}</p>
              <p>{t('automation.queuedNote')}</p>
            </div>
          </details>
        )}
      </div>
      {detailsOpen && (
        <div id={detailsId} className="automation-rule-panel">
          {a.definition.action.kind === 'prompt' ? (
            <>
              <p>{coverage}</p>
              {s &&
                s.ambiguousTurnCount +
                  s.missingTurnCount +
                  s.unattributedRunCount >
                  0 && (
                  <p>
                    {t('automation.incomplete', {
                      value1: s.ambiguousTurnCount,
                      value2: s.missingTurnCount,
                      value3: s.unattributedRunCount,
                    })}
                  </p>
                )}
            </>
          ) : pluginAction ? (
            <p>{t('automation.pluginCharge')}</p>
          ) : (
            <p>
              {t(
                a.definition.action.kind === 'runScript'
                  ? 'automation.scriptCharge'
                  : 'automation.inboxCharge',
              )}
            </p>
          )}
          {a.missedCount > 0 && (
            <p>{t('automation.mergedShort', { value1: a.missedCount })}</p>
          )}
          <details className="plugin-report-disclosure">
            <summary>{t('automation.definition')}</summary>
            <pre className="plugin-report-pre">
              {JSON.stringify(a.definition, null, 2)}
            </pre>
          </details>
        </div>
      )}
      {expanded && (
        <div id={historyId} className="automation-history">
          {error && !runsLoaded ? (
            <p className="automation-runs-empty flex items-center gap-1.5">
              {t('automation.unavailable')}
              <ErrorMark message={error} onRetry={() => retryRuns.current()} />
            </p>
          ) : (
            <RunHistory
              automation={a}
              runs={runs}
              threadId={threadId}
            />
          )}
        </div>
      )}
    </article>
  );
}

export function ThreadAutomationsControl({ thread }: { thread: ThreadDto }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<AutomationDto[]>([]);
  const [watches, setWatches] = useState<NativeWatch[]>([]);
  // Polls every 3 s. A failed poll keeps what was shown; only data that never
  // loaded says why (the availability row), and a failed Refresh is a mark
  // after the Refresh button.
  const [error, setError] = useState('');
  const [refreshError, setRefreshError] = useState('');
  const loaded = useRef({ rules: false, watches: false });
  const [unsupported, setUnsupported] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [loading, setLoading] = useState(true);
  const [nativeUnavailable, setNativeUnavailable] = useState(false);
  const [timezone, setTimezone] = useState('');
  const trigger = useRef<HTMLButtonElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const refreshRef = useRef<() => void>(() => {});
  const isClaude = thread.provider === 'claude' || thread.agentId === 'claude';
  useEffect(() => {
    setOpen(false);
    setItems([]);
    setWatches([]);
    setError('');
    setRefreshError('');
    loaded.current = { rules: false, watches: false };
    setUnsupported(false);
    setTimezone('');
    setLoading(true);
    setNativeUnavailable(false);
  }, [thread.id]);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    let inFlight = false;
    const update = async (manual = false) => {
      if (document.visibilityState === 'hidden' || inFlight) return;
      inFlight = true;
      setRefreshing(true);
      const results = await Promise.allSettled([
        request<{ automations: AutomationDto[] }>(
          `api/threads/${encodeURIComponent(thread.id)}/automations`,
          { signal: controller.signal, cache: 'no-store' },
        ),
        ...(isClaude
          ? [
              request<{ watches: NativeWatch[]; timezone?: string }>(
                `api/threads/${encodeURIComponent(thread.id)}/watches`,
                { signal: controller.signal, cache: 'no-store' },
              ),
            ]
          : []),
      ]);
      if (!controller.signal.aborted) {
        // Failures of data shown before: silent on a poll, a mark on Refresh.
        const stale: string[] = [];
        const rules = results[0];
        if (rules.status === 'fulfilled' && 'automations' in rules.value) {
          loaded.current.rules = true;
          setItems(rules.value.automations);
          setError('');
          setUnsupported(false);
        } else {
          const message = rules.status === 'rejected'
            ? String(rules.reason?.message ?? rules.reason)
            : t('automation.unavailable');
          if (loaded.current.rules) stale.push(message);
          else {
            setError(message);
            setUnsupported(
              rules.status === 'rejected' && (
                rules.reason?.statusCode === 404 ||
                /^Route not found$/i.test(message)),
            );
          }
        }
        const native = results[1];
        if (native?.status === 'fulfilled' && 'watches' in native.value) {
          loaded.current.watches = true;
          setWatches(native.value.watches);
          setTimezone(native.value.timezone ?? '');
          setNativeUnavailable(false);
        } else if (native?.status === 'rejected') {
          if (loaded.current.watches) stale.push(t('automation.nativeUnavailable'));
          else setNativeUnavailable(true);
        }
        if (manual || !stale.length) setRefreshError(stale.join('\n'));
        setLoading(false);
        setRefreshing(false);
      }
      inFlight = false;
    };
    refreshRef.current = () => void update(true);
    void update();
    const timer = window.setInterval(() => void update(), 3000);
    const visible = () => void update();
    document.addEventListener('visibilitychange', visible);
    return () => {
      controller.abort();
      refreshRef.current = () => {};
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [open, thread.id, isClaude, t]);
  const { locale } = useI18n();
  const totals = automationTotals(items, watches);
  const unavailable = Boolean(error || nativeUnavailable);
  const unknownEmpty = !items.length && !watches.length && unavailable;
  const active = items.filter(currentAutomation);
  const historical = items.filter((a) => !currentAutomation(a));
  const currentWatches = watches.filter(isCurrentWatch);
  const pastWatches = watches.filter((w) => !isCurrentWatch(w));
  const incompleteCounts = totals.unknownCounts > 0 || unavailable;
  const partialUsage =
    unavailable ||
    items.some(
      (a) =>
        a.definition.action.kind === 'pluginAction' ||
        !a.statistics ||
        !a.statistics.tokenUsage ||
        a.statistics.usageTurnCount < a.statistics.promptTurnCount ||
        a.statistics.ambiguousTurnCount +
          a.statistics.missingTurnCount +
          a.statistics.unattributedRunCount >
          0,
    ) ||
    watches.some(
      (w) =>
        !w.tokenUsage ||
        (w.usageTriggerCount ?? 0) < (w.triggerCount ?? 0) ||
        (w.ambiguousTriggerCount ?? 0) > 0,
    );
  const partialPrice =
    partialUsage ||
    items.some(
      (a) =>
        !a.statistics?.priceEstimate ||
        a.statistics.pricedTurnCount < a.statistics.promptTurnCount,
    ) ||
    watches.some(
      (w) =>
        !w.priceEstimate || (w.pricedTriggerCount ?? 0) < (w.triggerCount ?? 0),
    );
  const dash = loading || unknownEmpty;
  const compactNumber = (value: number) =>
    new Intl.NumberFormat(locale, {
      notation: 'compact',
      maximumFractionDigits: 1,
    }).format(value);
  const coverage = `${t('automation.totalCoverage')} ${t('automation.triggerSemantics')}`;
  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="matter-watches-toggle"
        aria-label={t('automation.title')}
        title={t('automation.title')}
        aria-expanded={open}
        onClick={() => setOpen(true)}
      >
        <Workflow data-icon="automations" size={14} />
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          className="automation-panel z-[100]"
          overlayClassName="z-[95] bg-[var(--overlay-scrim)]"
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            title.current?.focus();
          }}
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            trigger.current?.focus();
          }}
        >
          <DialogHeader className="automation-heading">
            <div className="automation-heading-icon">
              <Workflow size={16} strokeWidth={1.7} />
            </div>
            <div className="automation-heading-copy">
              <DialogTitle ref={title} tabIndex={-1} className="outline-none">
                {t('automation.title')}
              </DialogTitle>
              <DialogDescription>{t('automation.overview')}</DialogDescription>
            </div>
            <button
              type="button"
              className="automation-refresh"
              aria-label={t('automation.refresh')}
              title={t('automation.refresh')}
              disabled={refreshing}
              onClick={() => refreshRef.current()}
            >
              <RefreshCw
                size={15}
                className={refreshing ? 'animate-spin' : ''}
              />
            </button>
            {refreshError && <ErrorMark message={refreshError} onRetry={() => refreshRef.current()} />}
          </DialogHeader>
          <div className="automation-body" aria-busy={loading}>
            <section
              className="automation-metrics"
              aria-label={t('automation.totals')}
            >
              <Metric
                label={t('automation.activeCount')}
                value={
                  dash ? '—' : String(active.length + currentWatches.length)
                }
                accent={!dash && active.length + currentWatches.length > 0}
                partial={!dash && unavailable}
              />
              <Metric
                label={t('automation.triggerMetric')}
                value={
                  dash || (totals.triggers === 0 && incompleteCounts)
                    ? '—'
                    : totals.triggers.toLocaleString(locale)
                }
                partial={!dash && incompleteCounts}
              />
              <Metric
                label={t('automation.tokenMetric')}
                value={
                  dash || !totals.usage
                    ? '—'
                    : compactNumber(totals.usage.totalTokens)
                }
                title={totals.usage?.totalTokens.toLocaleString(locale)}
                partial={!dash && partialUsage}
              />
              <Metric
                label={t('automation.costMetric')}
                value={
                  dash || !totals.price ? (
                    '—'
                  ) : (
                    <TokenUsageCost
                      usage={totals.usage}
                      price={totals.price}
                      costLabel={t('automation.costMetric')}
                      detailsNote={coverage}
                      tooltipZIndex={120}
                    />
                  )
                }
                partial={!dash && partialPrice}
              />
            </section>
            {unavailable && (
              <div className="automation-availability" role="status">
                <CircleSlash size={16} />
                <span>
                  {t(
                    unsupported
                      ? 'automation.unsupported'
                      : 'automation.unavailable',
                  )}
                </span>
                <ErrorMark
                  kind={unsupported ? 'unavailable' : 'error'}
                  message={[error, nativeUnavailable ? t('automation.nativeUnavailable') : ''].filter(Boolean).join('\n')}
                  onRetry={unsupported ? undefined : () => refreshRef.current()}
                />
              </div>
            )}
            {loading ? (
              <div className="automation-empty" role="status">
                <RefreshCw size={20} className="animate-spin" />
                <p>{t('automation.loading')}</p>
              </div>
            ) : (
              <>
                {!items.length && !watches.length && !unavailable && (
                  <div className="automation-empty">
                    <div className="automation-empty-icon">
                      <Workflow size={26} strokeWidth={1.3} />
                    </div>
                    <h3>{t('automation.emptyTitle')}</h3>
                    <p>{t('automation.emptyHint')}</p>
                  </div>
                )}
                {(active.length > 0 || currentWatches.length > 0) && (
                  <section
                    className="automation-list"
                    aria-label={t('automation.active')}
                  >
                    <div className="automation-section-heading">
                      <h3>{t('automation.active')}</h3>
                      <span>{active.length + currentWatches.length}</span>
                    </div>
                    {active.map((a) => (
                      <AutomationCard
                        key={a.id}
                        automation={a}
                        threadId={thread.id}
                      />
                    ))}
                    {currentWatches.map((w) => (
                      <NativeWatchCard
                        key={`${w.id}:${w.createdAt}`}
                        watch={w}
                      />
                    ))}
                  </section>
                )}
                {(historical.length > 0 || pastWatches.length > 0) && (
                  <details className="automation-archive">
                    <summary>
                      <ChevronRight size={14} />
                      {t('automation.historical')} (
                      {historical.length + pastWatches.length})
                    </summary>
                    <div className="automation-list">
                      {historical.map((a) => (
                        <AutomationCard
                          key={a.id}
                          automation={a}
                          threadId={thread.id}
                        />
                      ))}
                      {pastWatches.map((w) => (
                        <NativeWatchCard
                          key={`${w.id}:${w.createdAt}`}
                          watch={w}
                        />
                      ))}
                    </div>
                  </details>
                )}
              </>
            )}
          </div>
          <footer className="automation-footer">
            <span>
              <Workflow size={12} />
              {t('automation.managed')}
            </span>
            <details className="automation-stat-notes">
              <summary>
                <Info size={13} />
                {t('automation.details')}
              </summary>
              <div>
                <p>{t('automation.totalCoverage')}</p>
                <p>{t('automation.triggerSemantics')}</p>
                {watches.length > 0 && (
                  <p>
                    {t('automation.native')} {timezone}
                  </p>
                )}
              </div>
            </details>
          </footer>
        </DialogContent>
      </Dialog>
    </>
  );
}

function Metric({
  label,
  value,
  title,
  partial,
  accent,
}: {
  label: string;
  value: ReactNode;
  title?: string | undefined;
  partial?: boolean;
  accent?: boolean;
}) {
  const { t } = useI18n();
  return (
    <div className="automation-metric" role="group" aria-label={label}>
      <span className="automation-metric-label">
        <span>{label}</span>
        {partial && (
          <span
            className="automation-metric-quality"
            title={t('automation.totalCoverage')}
          >
            {t('automation.partial')}
          </span>
        )}
      </span>
      <strong className={accent ? 'is-accent' : ''} title={title}>
        {value}
      </strong>
    </div>
  );
}
