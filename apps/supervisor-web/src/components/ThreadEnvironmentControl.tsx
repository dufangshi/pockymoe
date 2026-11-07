import { useCallback, useEffect, useState } from 'react';
import { Box, ChevronRight, Monitor, Plus, RefreshCw } from 'lucide-react';
import { ErrorMark, Select, StatusMark, getDirtyWorkspaceDocumentCount } from '@pockymoe/thread-ui';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import type { ThreadDto } from '@pockymoe/shared';
import {
  bindThreadEnvironment, createEnvironment, fetchEnvironmentRequests, fetchThreadEnvironment, notifyEnvironmentChanged,
  type EnvironmentDto, type EnvironmentRequestDto, type ThreadEnvironmentDto,
} from '../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../lib/environmentI18n';
import { environmentRequestText as rt } from '../lib/environmentRequestI18n';
import { EnvironmentWindow } from './environments/EnvironmentWindow';
import { EnvironmentFacts, EnvironmentLifecycle } from './environments/EnvironmentCard';
import { EnvironmentConfirmation } from './environments/EnvironmentConfirmation';
import { environmentError, useEnvironmentSnapshot } from './environments/model';
import { RequestList, useEnvironmentRequests } from './environments/EnvironmentRequests';
import { RequestReview } from './environments/RequestReview';
import { EnvironmentStateMark, SectionHeading, Tag, requestMark } from './environments/ui';

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

export function ThreadEnvironmentControl(props: Props) {
  useI18n();
  const [open, setOpen] = useState(false);
  const attention = useRequestAttention(props.thread.id, props.thread.status, props.deviceId, open);
  // The tools menu shows the accessible name, so a pending approval is visible before opening (UI §13).
  const label = t(menuLabel(attention));
  return <>
    <button type="button" aria-label={label} title={label} data-attention={attention ? requestMark(attention.state) : undefined} onClick={() => setOpen(true)}><Box aria-hidden="true" /></button>
    {open && <ThreadEnvironmentDialog {...props} onClose={() => setOpen(false)} />}
  </>;
}

/** One read when the thread's status or its environment changes; no polling while closed. */
function useRequestAttention(threadId: string, status: string, deviceId: string | null | undefined, open: boolean) {
  const [request, setRequest] = useState<EnvironmentRequestDto | undefined>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const changed = (event: Event) => { if ((event as CustomEvent<{ threadId?: string }>).detail?.threadId === threadId) setRevision(value => value + 1); };
    window.addEventListener('pockymoe:environment-changed', changed);
    return () => window.removeEventListener('pockymoe:environment-changed', changed);
  }, [threadId]);
  useEffect(() => {
    const controller = new AbortController();
    void fetchEnvironmentRequests(deviceId, threadId, controller.signal).then(requests => {
      if (!controller.signal.aborted) setRequest(activeRequest(requests ?? []));
    }).catch(() => { /* Older Supervisors and share recipients keep the plain entry. */ });
    return () => controller.abort();
  }, [threadId, status, deviceId, revision, open]);
  return request;
}

function ThreadEnvironmentDialog({ thread, deviceId, canManage, onClose }: Props & { onClose(): void }) {
  useI18n();
  const { snapshot, setSnapshot, loading, error: snapshotError, refresh } = useEnvironmentSnapshot(deviceId);
  const [binding, setBinding] = useState<ThreadEnvironmentDto | null>(null);
  const [bindingLoading, setBindingLoading] = useState(true);
  const [bindingError, setBindingError] = useState<string | null>(null);
  const [selected, setSelected] = useState('');
  const [createMode, setCreateMode] = useState(false);
  const [connectionId, setConnectionId] = useState('');
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [revision, setRevision] = useState(0);
  const [resetAction, setResetAction] = useState<'apply' | 'create' | null>(null);
  const [sessionReleased, setSessionReleased] = useState(false);
  const [reviewId, setReviewId] = useState<string | null>(null);
  const reload = useCallback(() => { void refresh(); setRevision(value => value + 1); }, [refresh]);
  const { requests, loading: requestsLoading, error: requestsError, refresh: refreshRequests, update: updateRequest } = useEnvironmentRequests(deviceId, thread.id, reload);
  useEffect(() => setSessionReleased(false), [thread.providerSessionId]);
  useEffect(() => {
    const controller = new AbortController();
    setBindingLoading(true); setBindingError(null);
    void fetchThreadEnvironment(thread.id, deviceId, controller.signal).then(next => {
      if (controller.signal.aborted) return;
      setBinding(previous => {
        setSelected(current => current === (previous?.environment?.id ?? '') ? next.environment?.id ?? '' : current);
        return next;
      });
    }).catch(cause => { if (!controller.signal.aborted) setBindingError(environmentError(cause, 'loadFailed')); })
      .finally(() => { if (!controller.signal.aborted) setBindingLoading(false); });
    return () => controller.abort();
  }, [thread.id, deviceId, thread.updatedAt, revision]);
  const updateEnvironment = useCallback((next: EnvironmentDto) => {
    setSnapshot(current => current && { ...current, environments: [...current.environments.filter(env => env.id !== next.id), next] });
    setBinding(current => current?.environment?.id === next.id ? { ...current, environment: next } : current);
  }, [setSnapshot]);
  const current = snapshot?.environments.find(env => env.id === binding?.environment?.id) ?? binding?.environment;
  const connection = snapshot?.connections.find(value => value.id === current?.connectionId);
  const activeThread = thread.status === 'running' || thread.status === 'recovering';
  const inheritedEnvironment = !!(thread.parentThreadId && binding?.environment);
  // Custom ACP threads use the existing "acp" provider; the environment API names that family "custom".
  const environmentFamily = thread.provider === 'acp' ? 'custom' : thread.provider;
  const supported = !!binding?.supportedHarnesses.some(harness => harness === thread.agentId || harness === thread.provider || harness === environmentFamily);
  const lifecycleReason = !canManage ? t('readOnly') : activeThread ? t('threadBusy') : inheritedEnvironment ? t('inheritedEnvironment') : undefined;
  const active = activeRequest(requests);
  // An approved migration owns the binding until it is retried or Host is restored from the request.
  const fenced = requests.some(request => ['waitingIdle', 'provisioning', 'failed'].includes(request.state)
    || (request.state === 'ready' && !!request.environmentId && request.environmentId === binding?.environment?.id));
  const reason = lifecycleReason ?? (fenced ? rt('fencedSwitch') : undefined) ?? (!supported && binding ? t('unsupportedHarness') : undefined);
  const available = snapshot?.environments.filter(env => (env.threadId === thread.id || env.id === current?.id) && env.state !== 'destroyed') ?? [];
  if (current && current.state !== 'destroyed' && !available.some(env => env.id === current.id)) available.push(current);
  const connections = snapshot?.connections.filter(value => value.configured) ?? [];
  const chosenConnection = connectionId || connections[0]?.id || '';
  const dirty = (!!binding && selected !== (binding.environment?.id ?? '')) || !!(name || connectionId);
  const unavailable = reason || snapshotError || bindingError;
  const apply = async (resetSession?: true) => {
    if (unavailable || busy || !binding) return;
    if (getDirtyWorkspaceDocumentCount()) { setError(t('saveFilesFirst')); return; }
    if (thread.providerSessionId && !sessionReleased && !resetSession) { setResetAction('apply'); return; }
    setBusy(true); setError(null); setSaved(false);
    try { const next = await bindThreadEnvironment(thread.id, selected || null, deviceId, resetSession); setBinding(next); setSelected(next.environment?.id ?? ''); setSaved(true); setResetAction(null); if (resetSession) setSessionReleased(true); notifyEnvironmentChanged(thread.id, deviceId); }
    catch (cause) { setError(environmentError(cause)); }
    finally { setBusy(false); }
  };
  const create = async (resetSession?: true) => {
    if (unavailable || busy || !chosenConnection || !binding) return;
    if (getDirtyWorkspaceDocumentCount()) { setError(t('saveFilesFirst')); return; }
    if (thread.providerSessionId && !sessionReleased && !resetSession) { setResetAction('create'); return; }
    setBusy(true); setError(null); setSaved(false);
    let created = false;
    try {
      const next = await createEnvironment({ connectionId: chosenConnection, threadId: thread.id, ...(name.trim() ? { name: name.trim() } : {}) }, deviceId);
      created = true; updateEnvironment(next); setSelected(next.id); setCreateMode(false); setName(''); setConnectionId('');
      const attached = await bindThreadEnvironment(thread.id, next.id, deviceId, resetSession);
      setBinding(attached); setSelected(attached.environment?.id ?? ''); setSaved(true); if (resetSession) setSessionReleased(true);
      notifyEnvironmentChanged(thread.id, deviceId);
    } catch (cause) { setError(created ? t('bindFailed') : environmentError(cause)); }
    finally { setBusy(false); setResetAction(null); }
  };
  const review = requests.find(request => request.id === reviewId);
  const refreshing = loading || bindingLoading;
  return <EnvironmentWindow title={t('title')} onClose={onClose} busy={busy} dirty={dirty}
    tools={<>
      {(snapshotError || bindingError) && <ErrorMark message={snapshotError || bindingError || ''} onRetry={() => { reload(); refreshRequests(); }} />}
      <button type="button" className="env-icon-button" aria-label={t('refresh')} title={t('refresh')} disabled={refreshing || busy}
        onClick={() => { reload(); refreshRequests(); }}><RefreshCw aria-hidden="true" className={refreshing ? 'animate-spin' : undefined} /></button>
    </>}>
    <div className="env-panel" data-testid="thread-environment">
      {refreshing && !binding && <p role="status" className="env-empty">{t('loading')}</p>}
      {binding && <section className="env-section" aria-labelledby="env-where">
        <SectionHeading id="env-where" title={t('current')} />
        <div className="env-group">
          {active && <ActiveRequest request={active} onOpen={() => setReviewId(active.id)} />}
          {current ? <>
            <div className="env-location" data-kind="isolated">
              <span className="env-location-icon" aria-hidden="true"><Box /></span>
              <span className="env-location-text"><strong>{current.name}</strong>
                <span>{connection?.name ?? 'Dokploy'} · {t('generationN', { n: formatNumber(current.generation) })}</span></span>
              <EnvironmentStateMark state={current.state} />
              {current.lastError && <ErrorMark message={t('safeError')} detail={current.lastError} announce={false} />}
            </div>
            <EnvironmentFacts environment={current} />
            <EnvironmentLifecycle key={current.id} environment={current} deviceId={deviceId} onUpdate={updateEnvironment} disabledReason={lifecycleReason} />
          </> : <div className="env-location" data-kind="host">
            <span className="env-location-icon" aria-hidden="true"><Monitor /></span>
            <span className="env-location-text"><strong>{t('hostTitle')}</strong><span>{t('hostDetail')}</span></span>
          </div>}
        </div>
      </section>}
      <section className="env-section" aria-labelledby="env-requests">
        <SectionHeading id="env-requests" title={rt('requests')} count={requests.length || undefined}>
          {requestsError && <ErrorMark message={requestsError} onRetry={refreshRequests} />}
        </SectionHeading>
        {requests.length ? <RequestList requests={requests} showThread={false} onOpen={setReviewId} />
          : !requestsLoading && !requestsError && <p className="env-empty">{rt('empty')}</p>}
      </section>
      {binding && snapshot && <section className="env-section" aria-labelledby="env-switch">
        <SectionHeading id="env-switch" title={t('switchHeading')} />
        {!createMode ? <div className="env-switch">
          <Select aria-label={t('select')} value={selected} disabled={busy || !!unavailable}
            onValueChange={value => { setSelected(value); setSaved(false); setError(null); }} options={[{ value: '', label: t('host') }, ...available.map(env => ({ value: env.id, label: env.name }))]} />
          <button type="button" className="relay-button-primary" disabled={busy || !!unavailable || selected === (binding.environment?.id ?? '')} onClick={() => void apply()}>{t(busy ? 'saving' : 'apply')}</button>
          <button type="button" className="relay-button-secondary" disabled={busy || !!unavailable || !connections.length}
            onClick={() => { setCreateMode(true); setSaved(false); setError(null); }}><Plus aria-hidden="true" />{t('create')}</button>
          {reason && <ErrorMark kind="unavailable" message={reason} />}
          {!connections.length && <ErrorMark kind="unavailable" message={t('setupHint')} />}
          {error && <ErrorMark message={error} />}
          {saved && <span role="status" className="env-ok">{t('applied')}</span>}
        </div> : <form className="env-create" onSubmit={event => { event.preventDefault(); void create(); }}>
          <p className="env-note">{t('createHint')}</p>
          <div className="env-create-fields">
            <label><span>{t('connection')}</span><Select aria-label={t('connection')} value={chosenConnection} disabled={busy || !!unavailable} onValueChange={setConnectionId}
              options={connections.map(value => ({ value: value.id, label: value.name }))} /></label>
            <label><span>{t('optionalName')}</span><input className="relay-input" value={name} disabled={busy} maxLength={120} onChange={event => setName(event.target.value)} /></label>
          </div>
          <div className="env-form-actions">
            {reason && <ErrorMark kind="unavailable" message={reason} />}{error && <ErrorMark message={error} />}
            <button type="button" className="relay-button-secondary" disabled={busy} onClick={() => setCreateMode(false)}>{t('cancel')}</button>
            <button type="submit" className="relay-button-primary" disabled={busy || !!unavailable || !chosenConnection}>{t(busy ? 'creatingAndBinding' : 'createAndUse')}</button>
          </div>
        </form>}
      </section>}
      {binding && <details className="env-technical">
        <summary><ChevronRight aria-hidden="true" />{t('scopeDetails')}{!!binding.limitations.length && <Tag>{formatNumber(binding.limitations.length)}</Tag>}</summary>
        <ul className="env-bullets">
          <li>{t('isolationHint')}</li>
          <li>{t('diskQuota')}</li>
          {binding.limitations.map(limitation => <li key={limitation}><span className="env-dim">{t('limitations')}:</span> {limitation}</li>)}
        </ul>
      </details>}
      <EnvironmentConfirmation open={resetAction !== null} title={t('resetTitle')} description={t('resetBody')} confirmLabel={t('resetConfirm')} busy={busy} busyLabel={t('working')}
        error={error} onCancel={() => { if (!busy) setResetAction(null); }} onConfirm={() => { if (resetAction === 'create') void create(true); else void apply(true); }} />
    </div>
    {review && <RequestReview request={review} deviceId={deviceId} canManage={canManage} onUpdate={updateRequest} onClose={() => setReviewId(null)} />}
  </EnvironmentWindow>;
}

/** The request that decides where this thread runs right now, with its next step. */
function ActiveRequest({ request, onOpen }: { request: EnvironmentRequestDto; onOpen(): void }) {
  const hint = request.state === 'pending' ? rt('lead', { connection: request.spec.connection.name })
    : request.state === 'failed' ? rt('fencedHint') : request.state === 'provisioning' ? rt('provisioningHint') : rt('waitingHint');
  return <div className="env-active" data-state={request.state}>
    <StatusMark status={requestMark(request.state)} label={rt(request.state)} />
    <span className="env-active-text"><strong>{rt(request.state)}</strong><span>{hint}</span></span>
    <button type="button" className={request.state === 'pending' ? 'relay-button-primary' : 'relay-button-secondary'} onClick={onOpen}>{rt(request.state === 'pending' ? 'review' : 'open')}</button>
  </div>;
}
