import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { ArrowLeft, Box, Check, ChevronRight, Info, Monitor, Plus, RefreshCw, Settings } from 'lucide-react';
import { ErrorMark, MiddleName, PathChip, Select, StatusMark, getDirtyWorkspaceDocumentCount, useAppShellNav } from '@pockymoe/thread-ui';
import { formatNumber, getLocale, useI18n } from '@pockymoe/thread-ui/i18n';
import type { ThreadDto } from '@pockymoe/shared';
import {
  actOnEnvironment, bindThreadEnvironment, createEnvironment, fetchEnvironmentRequests, fetchThreadEnvironment, notifyEnvironmentChanged,
  type EnvironmentConnectionDto, type EnvironmentDto, type EnvironmentRequestDto, type ThreadEnvironmentDto,
} from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';
import { environmentRequestText as rt } from '../../lib/environmentRequestI18n';
import { EnvironmentLifecycle } from './EnvironmentCard';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';
import { environmentError, environmentHoldsResources, migrationFailure, useEnvironmentSnapshot } from './model';
import { RequestList, useEnvironmentRequests } from './EnvironmentRequests';
import { RequestDecisionBar, RequestReviewDetails, RequestStatusLine, requestHasDecision, useRequestDecision } from './RequestReview';
import { EnvironmentStateMark, Fact, LimitChips, requestMark } from './ui';
import './environments.css';

type Props = {
  thread: Pick<ThreadDto, 'id' | 'status' | 'agentId' | 'provider' | 'providerSessionId' | 'updatedAt' | 'parentThreadId'>;
  deviceId?: string | null | undefined;
  canManage: boolean;
};

/** The request that currently decides this thread's execution, if any. */
const activeRequest = (requests: EnvironmentRequestDto[]) =>
  requests.find(request => request.state === 'failed') ?? requests.find(request => request.state === 'provisioning' || request.state === 'waitingIdle')
  ?? requests.find(request => request.state === 'pending');

const menuLabel = (request: EnvironmentRequestDto | undefined): EnvironmentMessage =>
  !request ? 'title' : request.state === 'pending' ? 'menuPending' : request.state === 'failed' ? 'menuFailed' : 'menuMigrating';

export const isolationMenuLabel = (request: EnvironmentRequestDto | undefined) => t(menuLabel(request));

/** One read when the thread's status or its environment changes; no polling while closed. */
export function useIsolationAttention(threadId: string | undefined, status: string | undefined, deviceId: string | null | undefined, open: boolean) {
  const [request, setRequest] = useState<EnvironmentRequestDto | undefined>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const changed = (event: Event) => { if ((event as CustomEvent<{ threadId?: string }>).detail?.threadId === threadId) setRevision(value => value + 1); };
    window.addEventListener('pockymoe:environment-changed', changed);
    return () => window.removeEventListener('pockymoe:environment-changed', changed);
  }, [threadId]);
  useEffect(() => {
    setRequest(undefined);
    if (!threadId) return;
    const controller = new AbortController();
    void fetchEnvironmentRequests(deviceId, threadId, controller.signal).then(requests => {
      if (!controller.signal.aborted) setRequest(activeRequest(requests ?? []));
    }).catch(() => { /* Older Supervisors and share recipients keep the plain entry. */ });
    return () => controller.abort();
  }, [threadId, status, deviceId, revision, open]);
  return request;
}

/** A Latin name inside a Chinese sentence gets spaces around it. */
const inline = (name: string) => getLocale() === 'zh-CN' ? ` ${name} ` : name;

type Step = 'creating' | 'switching' | 'starting';
type Target = { id: string | null; label: string };

/** The isolation inspector of one thread, docked beside the conversation. It
    answers three things at a glance: where the thread runs now (one status
    block), where it can run instead (a short list that acts on selection), and
    whether anything waits for the owner (requests). A request opens in place,
    with its decision pinned to the panel's foot. */
export function ThreadIsolationPanel({ thread, deviceId, canManage }: Props) {
  useI18n();
  const { snapshot, setSnapshot, loading, error: snapshotError, refresh } = useEnvironmentSnapshot(deviceId);
  const [binding, setBinding] = useState<ThreadEnvironmentDto | null>(null);
  const [bindingLoading, setBindingLoading] = useState(true);
  const [bindingError, setBindingError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [connectionId, setConnectionId] = useState('');
  const [name, setName] = useState('');
  const [step, setStep] = useState<Step | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [revision, setRevision] = useState(0);
  const [confirm, setConfirm] = useState<{ target: Target; run(): void } | null>(null);
  const [sessionReleased, setSessionReleased] = useState(false);
  const [reviewId, setReviewId] = useState<string | null>(null);
  const reload = useCallback(() => { void refresh(); setRevision(value => value + 1); }, [refresh]);
  const { requests, loading: requestsLoading, error: requestsError, refresh: refreshRequests, update: updateRequest } = useEnvironmentRequests(deviceId, thread.id, reload);
  useEffect(() => setSessionReleased(false), [thread.providerSessionId]);
  useEffect(() => {
    const controller = new AbortController();
    setBindingLoading(true); setBindingError(null);
    void fetchThreadEnvironment(thread.id, deviceId, controller.signal).then(next => {
      if (!controller.signal.aborted) setBinding(next);
    }).catch(cause => { if (!controller.signal.aborted) setBindingError(environmentError(cause, 'loadFailed')); })
      .finally(() => { if (!controller.signal.aborted) setBindingLoading(false); });
    return () => controller.abort();
  }, [thread.id, deviceId, thread.updatedAt, revision]);
  const updateEnvironment = useCallback((next: EnvironmentDto) => {
    setSnapshot(current => current && { ...current, environments: [...current.environments.filter(env => env.id !== next.id), next] });
    setBinding(current => current?.environment?.id === next.id ? { ...current, environment: next } : current);
  }, [setSnapshot]);

  const current = snapshot?.environments.find(env => env.id === binding?.environment?.id) ?? binding?.environment ?? null;
  const connectionOf = (env: EnvironmentDto | null | undefined) => snapshot?.connections.find(value => value.id === env?.connectionId);
  // A thread owns at most one environment that is not destroyed (the Supervisor
  // refuses a second); when it is not in use it is the other place to run.
  const own = snapshot?.environments.find(env => env.threadId === thread.id && env.state !== 'destroyed' && env.id !== current?.id) ?? null;
  const ownsOne = !!own || (!!current && current.threadId === thread.id && current.state !== 'destroyed');
  const connections = snapshot?.connections.filter(value => value.configured) ?? [];
  const chosenConnection = connectionId || connections[0]?.id || '';
  const activeThread = thread.status === 'running' || thread.status === 'recovering';
  const inherited = !!(thread.parentThreadId && binding?.environment && binding.environment.threadId !== thread.id);
  // Custom ACP threads use the existing "acp" provider; the environment API names that family "custom".
  const environmentFamily = thread.provider === 'acp' ? 'custom' : thread.provider;
  const supported = !!binding?.supportedHarnesses.some(harness => harness === thread.agentId || harness === thread.provider || harness === environmentFamily);
  const active = activeRequest(requests);
  const others = requests.filter(request => request !== active);
  // An approved migration owns the binding until it is retried or Host is restored from the request.
  const fenced = !!binding?.fenced || requests.some(request => ['waitingIdle', 'provisioning', 'failed'].includes(request.state)
    || (request.state === 'ready' && !!request.environmentId && request.environmentId === binding?.environment?.id));
  const ownerBlocked = binding?.ownerAuthorized === false;
  // The first reason that blocks every change, said in one line (no unexplained marks).
  const blocked: string | undefined = !canManage ? t('readOnly') : ownerBlocked ? t('viewOnly') : activeThread ? t('threadBusy')
    : inherited ? t('inheritedEnvironment') : fenced ? rt('fencedSwitch') : binding && !supported ? t('unsupportedHarness') : undefined;
  const lifecycleReason = !canManage ? t('readOnly') : ownerBlocked ? t('viewOnly') : activeThread ? t('threadBusy') : inherited ? t('inheritedEnvironment') : undefined;
  const loadFailed = snapshotError || bindingError;
  const busy = step !== null;
  const disabled = !!blocked || !!loadFailed || busy || !binding;

  /** Ask once before ending a live native session; nothing else interrupts work. */
  const guarded = (target: Target, run: (reset: boolean) => Promise<void>) => {
    if (disabled) return;
    if (getDirtyWorkspaceDocumentCount()) { setError(t('saveFilesFirst')); return; }
    setError(null); setSaved(false);
    if (thread.providerSessionId && !sessionReleased) setConfirm({ target, run: () => void run(true) });
    else void run(false);
  };
  const finish = (next: ThreadEnvironmentDto, reset: boolean) => {
    setBinding(next); setSaved(true); setConfirm(null);
    if (reset) setSessionReleased(true);
    notifyEnvironmentChanged(thread.id, deviceId);
  };
  /** Bound and stopped means turns queue silently: switching to it also starts it. */
  const startIfIdle = async (env: EnvironmentDto | null | undefined) => {
    if (!env || (env.state !== 'stopped' && env.state !== 'interrupted')) return;
    setStep('starting');
    updateEnvironment(await actOnEnvironment(env.id, 'start', deviceId));
  };
  const switchTo = (env: EnvironmentDto | null) => guarded({ id: env?.id ?? null, label: env?.name ?? t('thisDevice') }, async reset => {
    setStep('switching');
    try {
      const next = await bindThreadEnvironment(thread.id, env?.id ?? null, deviceId, reset || undefined);
      finish(next, reset);
      await startIfIdle(next.environment);
    } catch (cause) { setError(environmentError(cause)); }
    finally { setStep(null); }
  });
  const create = () => guarded({ id: 'new', label: name.trim() || t('newEnvironment') }, async reset => {
    if (!chosenConnection) return;
    setStep('creating');
    let created: EnvironmentDto | null = null;
    try {
      created = await createEnvironment({ connectionId: chosenConnection, threadId: thread.id, ...(name.trim() ? { name: name.trim() } : {}) }, deviceId);
      updateEnvironment(created); setCreateOpen(false); setName(''); setConnectionId('');
      setStep('switching');
      const next = await bindThreadEnvironment(thread.id, created.id, deviceId, reset || undefined);
      finish(next, reset);
      await startIfIdle(next.environment ?? created);
    } catch (cause) { setError(created ? t('bindFailed') : environmentError(cause)); }
    finally { setStep(null); }
  });

  const review = requests.find(request => request.id === reviewId);
  const refreshing = loading || bindingLoading;
  const root = useRef<HTMLDivElement>(null);
  const backTo = useRef<string | null>(null);
  useEffect(() => {
    // Back from a request returns focus to its row, not to the page top.
    if (review || !backTo.current) return;
    const id = backTo.current; backTo.current = null;
    root.current?.querySelector<HTMLElement>(`[data-request-id="${CSS.escape(id)}"] button`)?.focus({ preventScroll: false });
  }, [review]);
  if (review) return <RequestPane key={review.id} request={review} deviceId={deviceId} canManage={canManage} onUpdate={updateRequest}
    environment={snapshot?.environments.find(env => env.id === review.environmentId)}
    onBack={() => { backTo.current = review.id; setReviewId(null); }} />;

  const stepLabel = step === 'creating' ? t('stepCreating') : step === 'switching' ? t('stepSwitching') : step === 'starting' ? t('stepStarting') : null;
  return <div className="env-dock" ref={root} data-testid="thread-environment" aria-busy={busy || undefined}>
    <div className="env-dock-scroll">
      <div className="env-panel is-dock">
        {active && <ActiveRequest request={active} onOpen={() => setReviewId(active.id)} />}

        <section className="env-section" aria-labelledby="env-where">
          <header className="env-heading">
            <h3 id="env-where">{t('current')}</h3>
            <div className="env-heading-actions">
              {loadFailed && binding && <ErrorMark message={loadFailed} onRetry={() => { reload(); refreshRequests(); }} />}
              <button type="button" className="env-icon-button is-quiet" aria-label={t('refresh')} title={t('refresh')} disabled={refreshing || busy}
                onClick={() => { reload(); refreshRequests(); }}><RefreshCw aria-hidden="true" className={refreshing ? 'request-spin' : undefined} /></button>
            </div>
          </header>
          {refreshing && !binding && !loadFailed && <p role="status" className="env-empty">{t('loading')}</p>}
          {loadFailed && !binding && <p className="env-line is-error"><span>{loadFailed}</span>
            <button type="button" className="env-link" onClick={() => { reload(); refreshRequests(); }}>{t('retry')}</button></p>}
          {binding && <>
            {blocked && <p className="env-line" data-testid="environment-blocked"><Info aria-hidden="true" /><span>{blocked}</span></p>}
            {/* The current place comes first and carries the status block; the
                others act on a click (with one confirmation when a live session ends). */}
            <ul className="env-targets" aria-labelledby="env-where">
              {current
                ? <li aria-current="true" className="is-current"><CurrentEnvironment environment={current} connection={connectionOf(current)} deviceId={deviceId} onUpdate={updateEnvironment} disabledReason={lifecycleReason} /></li>
                : <TargetRow current icon={<Monitor />} title={t('hostTitle')} meta={<span className="env-dim">{t('notIsolated')}</span>} />}
              {current && <TargetRow current={false} disabled={disabled || inherited} onSelect={() => switchTo(null)}
                icon={<Monitor />} title={t('hostTitle')} meta={<span className="env-dim">{t('notIsolated')}</span>} label={t('switchTo', { name: t('thisDevice') })} />}
              {own && <TargetRow current={false} disabled={disabled || own.state === 'error'} onSelect={() => switchTo(own)} icon={<Box />} title={own.name}
                meta={<EnvironmentStateMark state={own.state} />} label={t('switchTo', { name: inline(own.name) })}
                note={environmentHoldsResources(own.state) && own.state !== 'error' ? <OwnLifecycle environment={own} deviceId={deviceId} onUpdate={updateEnvironment} disabled={!!lifecycleReason || busy} /> : undefined} />}
              {!ownsOne && !inherited && (connections.length
                ? !createOpen
                  ? <li><button type="button" className="env-target is-new" disabled={disabled} onClick={() => { setCreateOpen(true); setSaved(false); setError(null); }}>
                    <span className="env-target-icon" aria-hidden="true"><Plus /></span>
                    <span className="env-target-main"><span className="env-target-title">{t('newEnvironment')}</span>
                      <span className="env-target-meta">{connections.length === 1 ? connections[0]!.name : t('connection')}</span></span>
                  </button></li>
                  : <li><CreateForm connections={connections} connectionId={chosenConnection} onConnection={setConnectionId} name={name} onName={setName}
                    busy={busy} disabled={disabled} onCancel={() => setCreateOpen(false)} onSubmit={create} /></li>
                : snapshot && <li className="env-target is-missing">
                  <span className="env-target-icon" aria-hidden="true"><Plus /></span>
                  <span className="env-target-main"><span className="env-target-title">{t('newEnvironment')}</span>
                    <span className="env-target-meta">{t('needsConnection')}</span></span>
                  <SetUpLink />
                </li>)}
            </ul>
            {(stepLabel || error || saved) && <p className="env-line is-feedback" role="status">
              {stepLabel ? <span className="env-dim">{stepLabel}</span> : saved && !error ? <span className="env-ok"><Check aria-hidden="true" />{t('switched')}</span> : null}
              {error && <ErrorMark message={error} />}
            </p>}
          </>}
        </section>

        {/* The request in the banner above is not listed twice; with nothing
            else, the section is one short line, or gone when the banner says it all. */}
        {(others.length > 0 || !active || requestsError) && <section className="env-section" aria-labelledby="env-requests">
          <header className="env-heading">
            <h3 id="env-requests">{rt('requests')}</h3>
            {requestsError && <div className="env-heading-actions"><ErrorMark message={requestsError} onRetry={refreshRequests} /></div>}
          </header>
          {others.length ? <RequestList requests={others} showThread={false} onOpen={setReviewId} />
            : !requestsLoading && !requestsError && !active && <p className="env-empty">{rt('empty')}</p>}
        </section>}

        {binding && <ScopeDetails binding={binding} />}
        <EnvironmentConfirmation open={confirm !== null} title={t('resetTitle')}
          description={t('resetBody', { target: confirm?.target.id === null ? t('thisDevice') : inline(confirm?.target.label ?? '') })}
          confirmLabel={t('resetConfirm')} tone="primary" busy={busy} busyLabel={stepLabel ?? t('working')}
          error={error} onCancel={() => { if (!busy) setConfirm(null); }} onConfirm={() => confirm?.run()} />
      </div>
    </div>
  </div>;
}

/** Where the thread runs now: name and state, the image it runs, its limits as
    labelled chips, what its state means for turns, and its lifecycle. Paths,
    generation and enforced limits fold under Details. */
function CurrentEnvironment({ environment: env, connection, deviceId, onUpdate, disabledReason }: {
  environment: EnvironmentDto; connection: EnvironmentConnectionDto | undefined; deviceId: string | null | undefined;
  onUpdate(env: EnvironmentDto): void; disabledReason: string | undefined;
}) {
  const hint: EnvironmentMessage | null = env.state === 'stopped' ? 'stateHintStopped' : env.state === 'interrupted' ? 'stateHintInterrupted'
    : env.state === 'error' ? 'stateHintError' : env.state === 'destroyed' ? 'stateHintDestroyed' : null;
  return <div className="env-now-card" data-state={env.state}>
    <div className="env-now-head" data-kind="isolated">
      <span className="env-now-icon" aria-hidden="true"><Box /></span>
      <span className="env-now-title"><strong title={env.name}>{env.name}</strong>
        <span className="env-now-sub">{connection?.name ?? 'Dokploy'}{connection?.image && <> · <code title={connection.image}><MiddleName name={connection.image} /></code></>}</span></span>
      <EnvironmentStateMark state={env.state} />
    </div>
    <LimitChips limits={env.limits} observed={env.observedLimits} />
    {hint && <p className="env-line" data-state={env.state}><span>{t(hint)}</span>
      {env.lastError && <ErrorMark message={t('safeError')} detail={env.lastError} announce={false} />}</p>}
    <EnvironmentLifecycle key={env.id} environment={env} deviceId={deviceId} onUpdate={onUpdate} disabledReason={disabledReason} />
    <details className="env-technical">
      <summary><ChevronRight aria-hidden="true" />{t('details')}</summary>
      <dl className="env-facts is-compact">
        <Fact label={t('workspace')}><PathChip path={env.workspacePath} label={t('copyAbsolutePath')} /></Fact>
        {env.worktreePath && env.worktreePath !== env.workspacePath && <Fact label={t('worktree')}><PathChip path={env.worktreePath} label={t('copyAbsolutePath')} /></Fact>}
        <Fact label={t('observedLimits')}>{env.observedLimits ? <span className="env-ok">{t('observedVerified')}</span> : <span className="env-dim">{t('notObserved')}</span>}</Fact>
        <Fact label={t('generation')}>{formatNumber(env.generation)}</Fact>
      </dl>
    </details>
  </div>;
}

/** One place the thread can run. The current one is marked, not a button. */
function TargetRow({ current, disabled = false, onSelect, icon, title, meta, label, note }: {
  current: boolean; disabled?: boolean; onSelect?(): void; icon: ReactNode; title: string; meta: ReactNode; label?: string; note?: ReactNode;
}) {
  // The current place states its condition on the right (like an environment's
  // state); the others say what a click does.
  const body = current ? <>
    <span className="env-target-icon" aria-hidden="true">{icon}</span>
    <span className="env-target-main"><span className="env-target-title" title={title}>{title}</span></span>
    <span className="env-pill">{meta}</span>
  </> : <>
    <span className="env-target-icon" aria-hidden="true">{icon}</span>
    <span className="env-target-main"><span className="env-target-title" title={title}>{title}</span><span className="env-target-meta">{meta}</span></span>
    <span className="env-target-go" aria-hidden="true">{t('switchAction')}<ChevronRight /></span>
  </>;
  return <li aria-current={current || undefined} className={current ? 'is-current' : undefined}>
    {current ? <div className="env-target is-current">{body}</div>
      : <button type="button" className="env-target" disabled={disabled} aria-label={label} onClick={onSelect}>{body}</button>}
    {note}
  </li>;
}

/** An environment not in use that still holds resources: say so and offer Stop. */
function OwnLifecycle({ environment: env, deviceId, onUpdate, disabled }: { environment: EnvironmentDto; deviceId: string | null | undefined; onUpdate(env: EnvironmentDto): void; disabled: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (env.state !== 'running' && env.state !== 'interrupted') return null;
  const stop = async () => {
    setBusy(true); setError(null);
    try { onUpdate(await actOnEnvironment(env.id, 'stop', deviceId)); notifyEnvironmentChanged(env.threadId, deviceId); }
    catch (cause) { setError(environmentError(cause)); }
    finally { setBusy(false); }
  };
  return <p className="env-target-note"><span>{t('stillRunning')}</span>
    <button type="button" className="env-button is-small" disabled={disabled || busy} onClick={() => void stop()}>{busy ? t('working') : t('stop')}</button>
    {error && <ErrorMark message={error} />}</p>;
}

function CreateForm({ connections, connectionId, onConnection, name, onName, busy, disabled, onCancel, onSubmit }: {
  connections: EnvironmentConnectionDto[]; connectionId: string; onConnection(id: string): void; name: string; onName(name: string): void;
  busy: boolean; disabled: boolean; onCancel(): void; onSubmit(): void;
}) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { input.current?.focus(); }, []);
  const connection = connections.find(value => value.id === connectionId);
  return <form className="env-create" aria-label={t('newEnvironment')} onSubmit={event => { event.preventDefault(); onSubmit(); }}
    onKeyDown={event => { if (event.key === 'Escape' && !busy) { event.stopPropagation(); onCancel(); } }}>
    <label className="env-create-name"><span className="sr-only">{t('optionalName')}</span>
      <input ref={input} className="relay-input" value={name} disabled={busy} maxLength={120} placeholder={t('optionalName')} aria-label={t('optionalName')}
        onChange={event => onName(event.target.value)} /></label>
    {connections.length > 1 && <Select aria-label={t('connection')} value={connectionId} disabled={busy} onValueChange={onConnection}
      options={connections.map(value => ({ value: value.id, label: value.name }))} />}
    {connection && <p className="env-create-note">{t('createHint')}</p>}
    <div className="env-form-actions">
      <button type="button" className="relay-button-secondary" disabled={busy} onClick={onCancel}>{t('cancel')}</button>
      <button type="submit" className="relay-button-primary" disabled={disabled || !connectionId}>{t('createAndSwitch')}</button>
    </div>
  </form>;
}

/** Settings → Isolated environments, where connections are added. */
function SetUpLink() {
  const nav = useAppShellNav();
  if (!nav?.openSettingsSection) return null;
  return <button type="button" className="env-button is-small" onClick={() => nav.openSettingsSection?.('environments')}><Settings aria-hidden="true" />{t('setUp')}</button>;
}

const SCOPE: Record<string, [EnvironmentMessage, EnvironmentMessage][]> = {
  credentialsByRequest: [['scopeCredentials', 'scopeCredentialsValue'], ['scopeUnsupported', 'scopeUnsupportedValue']],
  noDiskQuota: [['scopeDisk', 'scopeDiskValue']],
  capabilityHourly: [['scopeAccess', 'scopeAccessValue']],
};
/** What isolation covers and what it does not, as labelled lines. Facts the
    Supervisor reports by key are said in the reader's language; any other
    limitation it adds is listed as sent. */
function ScopeDetails({ binding }: { binding: ThreadEnvironmentDto }) {
  const keys = binding.limitationKeys ?? [];
  const known = keys.length === binding.limitations.length ? keys.flatMap(key => SCOPE[key] ?? []) : [];
  const other = keys.length === binding.limitations.length ? binding.limitations.filter((_, index) => !SCOPE[keys[index]!]) : binding.limitations;
  const items: [EnvironmentMessage, EnvironmentMessage][] = [['scopeNetwork', 'scopeNetworkValue'], ...known, ['scopeSkills', 'scopeSkillsValue']];
  return <details className="env-technical env-scope">
    <summary><ChevronRight aria-hidden="true" />{t('scopeDetails')}</summary>
    <dl className="env-facts is-compact">
      {items.map(([label, value]) => <Fact key={label} label={t(label)}>{t(value)}</Fact>)}
      {other.map(text => <Fact key={text} label={t('scopeOther')}>{text}</Fact>)}
    </dl>
  </details>;
}

/** A request opened in the panel: its review scrolls, its decision stays at the foot. */
function RequestPane({ request, deviceId, canManage, onUpdate, onBack, environment }: {
  request: EnvironmentRequestDto; deviceId: string | null | undefined; canManage: boolean;
  onUpdate(request: EnvironmentRequestDto): void; onBack(): void; environment?: EnvironmentDto | undefined;
}) {
  useI18n();
  const decision = useRequestDecision(request, deviceId, canManage, onUpdate, environment);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  return <div className="env-dock is-detail" data-testid="isolation-request" aria-busy={decision.busy ? true : undefined}>
    <div className="env-dock-bar">
      <button type="button" className="env-back" disabled={!!decision.busy} onClick={onBack}><ArrowLeft aria-hidden="true" />{rt('requests')}</button>
    </div>
    <div className="env-dock-scroll">
      <header className="env-dock-title">
        <h3 ref={heading} tabIndex={-1}>{request.spec.name}</h3>
        <RequestStatusLine request={request} />
      </header>
      <RequestReviewDetails request={request} />
    </div>
    {requestHasDecision(request.state) && <footer className="env-dock-footer"><RequestDecisionBar decision={decision} /></footer>}
  </div>;
}

/** The request that decides where this thread runs right now, with its next step. */
function ActiveRequest({ request, onOpen }: { request: EnvironmentRequestDto; onOpen(): void }) {
  const hint = request.state === 'pending' ? rt('lead', { connection: request.spec.connection.name })
    : request.state === 'failed' ? t(migrationFailure(request.lastError)) : request.state === 'provisioning' ? rt('provisioningHint') : rt('waitingHint');
  return <div className="env-active" data-state={request.state} data-request-id={request.id}>
    <StatusMark status={requestMark(request.state)} label={rt(request.state)} />
    <span className="env-active-text"><strong>{rt(request.state)}</strong><span>{hint}</span></span>
    <button type="button" className={request.state === 'pending' || request.state === 'failed' ? 'relay-button-primary' : 'relay-button-secondary'} onClick={onOpen}>{rt(request.state === 'pending' ? 'review' : request.state === 'failed' ? 'resolve' : 'open')}</button>
  </div>;
}
