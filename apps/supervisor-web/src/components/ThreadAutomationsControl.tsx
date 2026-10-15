import { useEffect, useRef, useState, type ReactNode } from 'react';
import {
  Workflow,
  RefreshCw,
  Clock3,
  ChevronRight,
  Info,
  MessageSquare,
  Inbox,
  Terminal,
  CircleSlash,
  ArrowUpRight,
} from 'lucide-react';
import './automation-panel.css';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  TokenUsageCost,
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

const button =
  'rounded border border-[var(--theme-border)] px-2.5 py-1.5 text-xs hover:bg-[var(--theme-bg)]';
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
      usage: a.statistics?.tokenUsage,
      price: a.statistics?.priceEstimate,
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
    <div className="automation-rule-usage">
      <span>
        {t('automation.tokens')}:{' '}
        {usage?.totalTokens.toLocaleString(locale) ?? t('automation.unknown')}
      </span>
      <span>
        {t('automation.cost')}:{' '}
        {usage || price ? (
          <TokenUsageCost
            usage={usage ?? null}
            price={price ?? null}
            costLabel={t('automation.cost')}
            detailsNote={note ?? ''}
            tooltipZIndex={120}
          />
        ) : (
          t('automation.unknown')
        )}
      </span>
    </div>
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
  const [runs, setRuns] = useState<AutomationRunDto[]>([]);
  const [output, setOutput] = useState('');
  const [error, setError] = useState('');
  const base = `api/threads/${encodeURIComponent(threadId)}/automations/${encodeURIComponent(a.id)}`;
  const date = (v: string | null) =>
    v ? new Date(v).toLocaleString(locale) : t('automation.none');
  const state = (v: string) =>
    knownStates.has(v) ? t(`automation.${v}` as TranslationKey) : v;
  const triggerLabel = (v: AutomationTrigger) =>
    v.kind === 'interval'
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
        ? date(v.at)
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
          setError('');
        }
      } catch (e) {
        if (!controller.signal.aborted)
          setError(String(e instanceof Error ? e.message : e));
      } finally {
        refreshing = false;
      }
    };
    void update();
    const timer = window.setInterval(() => void update(), 3000);
    return () => {
      controller.abort();
      window.clearInterval(timer);
    };
  }, [expanded, base]);
  const s = a.statistics;
  const coverage = s
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
        <span className={`automation-state is-${a.state}`}>
          <i />
          {state(a.state)}
        </span>
      </div>
      <div className="automation-rule-schedule">
        <Clock3 size={13} />
        {triggerLabel(a.definition.trigger)}
        <span>·</span>
        {a.definition.action.kind === 'prompt' ? (
          <MessageSquare size={13} />
        ) : a.definition.action.kind === 'notifyInbox' ? (
          <Inbox size={13} />
        ) : (
          <Terminal size={13} />
        )}
        {t(`automation.${a.definition.action.kind}`)}
      </div>
      <div className="automation-rule-meta">
        <span>
          {t('automation.triggers')}:{' '}
          {s?.triggerCount.toLocaleString(locale) ?? '—'}
        </span>
        {a.pendingCount > 0 && (
          <span>
            {t('automation.pendingShort', { value1: a.pendingCount })}
          </span>
        )}
        {a.nextRunAt && (
          <span title={date(a.nextRunAt)}>
            {t('automation.next')}: {date(a.nextRunAt)}
          </span>
        )}
      </div>
      <Usage usage={s?.tokenUsage} price={s?.priceEstimate} note={coverage} />
      <details className="automation-rule-details">
        <summary>
          <ChevronRight size={13} />
          {t('automation.ruleDetails')}
        </summary>
        <div>
          {a.definition.action.kind === 'prompt' ? (
            <>
              <p className="text-xs text-[var(--theme-fg-muted)]">{coverage}</p>
              {s &&
                s.ambiguousTurnCount +
                  s.missingTurnCount +
                  s.unattributedRunCount >
                  0 && (
                  <p className="text-xs text-[var(--theme-fg-muted)]">
                    {t('automation.incomplete', {
                      value1: s.ambiguousTurnCount,
                      value2: s.missingTurnCount,
                      value3: s.unattributedRunCount,
                    })}
                  </p>
                )}
            </>
          ) : (
            <p className="text-xs text-[var(--theme-fg-muted)]">
              {t(
                a.definition.action.kind === 'runScript'
                  ? 'automation.scriptCharge'
                  : 'automation.inboxCharge',
              )}
            </p>
          )}
          {a.error && (
            <p className="break-words text-xs" role="status">
              {t('automation.error')}: {a.error}
            </p>
          )}
          {a.missedCount > 0 && (
            <p className="text-xs">
              {t('automation.mergedShort', { value1: a.missedCount })}
            </p>
          )}
          <details>
            <summary className="cursor-pointer text-xs">
              {t('automation.definition')}
            </summary>
            <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap break-all text-xs">
              {JSON.stringify(a.definition, null, 2)}
            </pre>
          </details>
        </div>
      </details>
      <button
        type="button"
        className="automation-history-button"
        aria-expanded={expanded}
        onClick={() => setExpanded(!expanded)}
      >
        {t('automation.history')}
        <ArrowUpRight size={12} />
      </button>
      {expanded && (
        <div className="space-y-2 border-t border-[var(--theme-border)] pt-2">
          <p className="text-xs text-[var(--theme-fg-muted)]">
            {t('automation.historyLimit')} {t('automation.queuedNote')}
          </p>
          {error && (
            <p role="alert" className="break-words text-xs">
              {error}
            </p>
          )}
          {!runs.length && <p className="text-xs">{t('automation.noRuns')}</p>}
          {runs.map((r) => (
            <section
              key={r.id}
              className="space-y-1 rounded bg-[var(--theme-bg)] p-2 text-xs"
            >
              <strong>{state(r.state)}</strong> · {t('automation.merged')}:{' '}
              {r.missedCount}
              <p>
                {t('automation.scheduled')}: {date(r.scheduledAt)}
              </p>
              <p>
                {t('automation.started')}: {date(r.startedAt)} ·{' '}
                {t('automation.finished')}: {date(r.completedAt)}
              </p>
              {r.turnId && (
                <p className="break-all">
                  {t('automation.turn')}: {r.turnId}
                </p>
              )}
              {r.error && (
                <p className="break-words" role="status">
                  {r.error}
                </p>
              )}
              {r.commandId && (
                <button
                  type="button"
                  className={button}
                  onClick={() => {
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
                      .catch((e) => setError(String(e.message ?? e)));
                  }}
                >
                  {t('automation.output')}
                </button>
              )}
            </section>
          ))}
          {output && (
            <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all text-xs">
              {output}
            </pre>
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
  const [error, setError] = useState('');
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
    setUnsupported(false);
    setTimezone('');
    setLoading(true);
    setNativeUnavailable(false);
  }, [thread.id]);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    let inFlight = false;
    const update = async () => {
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
        const rules = results[0];
        if (rules.status === 'fulfilled' && 'automations' in rules.value) {
          setItems(rules.value.automations);
          setError('');
          setUnsupported(false);
        } else if (rules.status === 'rejected') {
          const message = String(rules.reason?.message ?? rules.reason);
          setError(message);
          setUnsupported(
            rules.reason?.statusCode === 404 ||
              /^Route not found$/i.test(message),
          );
        } else {
          setError(t('automation.unavailable'));
        }
        const native = results[1];
        if (native?.status === 'fulfilled' && 'watches' in native.value) {
          setWatches(native.value.watches);
          setTimezone(native.value.timezone ?? '');
          setNativeUnavailable(false);
        } else if (native?.status === 'rejected') setNativeUnavailable(true);
        setLoading(false);
        setRefreshing(false);
      }
      inFlight = false;
    };
    refreshRef.current = () => void update();
    void update();
    const timer = window.setInterval(() => void update(), 3000);
    document.addEventListener('visibilitychange', update);
    return () => {
      controller.abort();
      refreshRef.current = () => {};
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', update);
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
              <Workflow size={20} strokeWidth={1.6} />
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
                size={16}
                className={refreshing ? 'animate-spin' : ''}
              />
            </button>
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
              <div
                className="automation-availability"
                role={error && !unsupported ? 'alert' : 'status'}
              >
                <CircleSlash size={16} />
                <span>
                  {t(
                    unsupported
                      ? 'automation.unsupported'
                      : 'automation.unavailable',
                  )}
                </span>
                <details>
                  <summary
                    aria-label={t('automation.diagnostics')}
                    title={t('automation.diagnostics')}
                  >
                    <Info size={14} />
                  </summary>
                  <div>
                    {error && <p>{error}</p>}
                    {nativeUnavailable && (
                      <p>{t('automation.nativeUnavailable')}</p>
                    )}
                  </div>
                </details>
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
      <span className="automation-metric-label">{label}</span>
      <strong className={accent ? 'is-accent' : ''} title={title}>
        {value}
      </strong>
      <span className="automation-metric-quality">
        {partial && (
          <span title={t('automation.totalCoverage')}>
            {t('automation.partial')}
          </span>
        )}
      </span>
    </div>
  );
}
