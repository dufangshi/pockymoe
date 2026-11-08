import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { CopyButton, ErrorMark, PathChip, StatusMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { decideEnvironmentRequest, fetchEnvironmentRequests, notifyEnvironmentChanged, type EnvironmentRequestDto } from '../../lib/environmentApi';
import { environmentText as et } from '../../lib/environmentI18n';
import { environmentRequestText as t } from '../../lib/environmentRequestI18n';
import { Limits } from './EnvironmentCard';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';
import './environments.css';

export function EnvironmentRequests({ deviceId, threadId, canManage = true, onChanged }: {
  deviceId?: string | null | undefined; threadId?: string; canManage?: boolean; onChanged?(): void;
}) {
  useI18n();
  const [requests, setRequests] = useState<EnvironmentRequestDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const previous = useRef(new Map<string, string>());
  const changed = useRef(onChanged); changed.current = onChanged;
  useEffect(() => {
    const controller = new AbortController(); let polling = false;
    const load = async () => {
      if (polling) return; polling = true;
      try {
        const next = await fetchEnvironmentRequests(deviceId, threadId, controller.signal);
        if (controller.signal.aborted) return;
        for (const request of next) {
          const before = previous.current.get(request.id);
          if (before && before !== request.state) {
            notifyEnvironmentChanged(request.threadId, deviceId); changed.current?.();
          }
        }
        previous.current = new Map(next.map(request => [request.id, request.state]));
        setRequests(next); setError(null);
      } catch { if (!controller.signal.aborted) setError(t('loadFailed')); }
      finally { polling = false; if (!controller.signal.aborted) setLoading(false); }
    };
    void load(); const timer = window.setInterval(() => void load(), 3000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [deviceId, threadId, revision]);
  const update = useCallback((next: EnvironmentRequestDto) => {
    setRequests(current => current.map(request => request.id === next.id ? next : request));
    previous.current.set(next.id, next.state); notifyEnvironmentChanged(next.threadId, deviceId); changed.current?.();
  }, [deviceId]);
  return <section className="environment-section" aria-label={t('requests')} data-testid="environment-requests">
    <header><h4>{t('requests')}</h4><button type="button" className="host-secondary-button" onClick={() => setRevision(value => value + 1)}>{t('refreshList')}</button></header>
    <p className="environment-hint">{t('hint')}</p>
    {loading && <p role="status">{t('loading')}</p>}
    {error && <ErrorMark message={error} onRetry={() => setRevision(value => value + 1)} />}
    {!loading && !error && !requests.length && <p className="environment-hint">{t('empty')}</p>}
    {requests.slice().sort((a, b) => b.createdAt.localeCompare(a.createdAt)).map(request => <RequestCard key={request.id} request={request} deviceId={deviceId} canManage={canManage} onUpdate={update} />)}
  </section>;
}

function RequestCard({ request, deviceId, canManage, onUpdate }: { request: EnvironmentRequestDto; deviceId?: string | null | undefined; canManage: boolean; onUpdate(request: EnvironmentRequestDto): void }) {
  const [expanded, setExpanded] = useState(request.state === 'pending' || request.state === 'failed');
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const [recover, setRecover] = useState(false);
  const spec = request.spec;
  const mark = request.state === 'pending' ? 'waiting' : request.state === 'failed' ? 'failed' : ['waitingIdle', 'provisioning'].includes(request.state) ? 'running' : 'idle';
  const act = async (action: 'approve' | 'reject' | 'retry' | 'recoverHost') => {
    if (busy || !canManage) return; setBusy(true); setError(null);
    try { onUpdate(await decideEnvironmentRequest(request, action, deviceId)); setRecover(false); }
    catch { setError(t('decisionFailed')); }
    finally { setBusy(false); }
  };
  return <article className="environment-card environment-request" aria-label={spec.name} data-request-id={request.id}>
    <div className="environment-card-heading"><button type="button" className="environment-expand" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>
      <ChevronRight aria-hidden className={expanded ? 'is-expanded' : ''} /><StatusMark status={mark} label={t(request.state)} /><span>{spec.name}</span>
    </button><span role="status" className="environment-hint">{t(request.state)}</span></div>
    {expanded && <div className="environment-card-details">
      <p>{spec.connection.name} · Dokploy</p>
      <dl className="environment-stats"><div><dt>{t('identity')}</dt><dd className="environment-request-id">{request.threadId}<CopyButton value={request.threadId} label={t('copyThread')} /></dd></div>
        <div><dt>{t('model')}</dt><dd>{spec.harness} · {spec.model ?? 'default'} · {spec.reasoningEffort ?? 'default'}</dd></div></dl>
      <div className="environment-path"><span>{et('workspace')}</span><PathChip path={spec.workspacePath} label={et('copyAbsolutePath')} /></div>
      <div className="environment-path"><span>{et('worktree')}</span><PathChip path={spec.workingPath} label={et('copyAbsolutePath')} /></div>
      <h5>{et('requestedLimits')}</h5><Limits value={spec.limits} /><p className="environment-hint">{et('diskQuota')}</p>
      <dl><dt>Codex</dt><dd>{t(spec.authentication.codex === 'nativeFile' ? 'native' : 'broker')}</dd><dt>Claude</dt><dd>{t(spec.authentication.claude === 'nativeFile' ? 'native' : 'broker')}</dd></dl>
      <h5>{t('credentials')}</h5>
      {!spec.credentialMounts.length ? <p className="environment-hint">{t('none')}</p> : <>
        <p className="environment-hint">{t('direct')}</p>
        {spec.credentialMounts.map((mount, index) => <div key={mount.target} className="environment-credential-mount">
          <strong>{mount.purpose === 'github' ? 'GitHub CLI' : mount.purpose === 'generic' ? t('credentials') : mount.purpose === 'codex' ? 'Codex' : 'Claude'} · {t(spec.credentialMountKinds[index] === 'directory' ? 'directory' : 'file')} · {t(mount.readOnly ? 'readOnly' : 'readWrite')}</strong>
          <div className="environment-path"><span>{t('source')}</span><PathChip path={mount.source} label={et('copyAbsolutePath')} /></div>
          <div className="environment-path"><span>→ {t('target')}</span><PathChip path={mount.target} label={et('copyAbsolutePath')} /></div>
          {Object.entries(mount.environment).map(([key, path]) => <div key={key} className="environment-path"><code>{key}</code><PathChip path={path} label={et('copyAbsolutePath')} /></div>)}
          <p className="environment-hint">{t(mount.readOnly ? (spec.credentialMountKinds[index] === 'file' ? 'readOnlyHint' : 'readOnlyDirectory') : (mount.purpose === 'codex' || mount.purpose === 'claude' ? 'refresh' : 'writableHint'))}</p>
        </div>)}
        <p className="environment-hint">{t('stores')}</p>
      </>}
      <h5>{t('network')}</h5>{spec.egressHosts.length ? <><ul>{spec.egressHosts.map(host => <li key={host}><code>{host}</code></li>)}</ul><p className="environment-hint">{t('networkHint')}</p></> : <p className="environment-hint">{t('offline')}</p>}
      <h5>{t('migration')}</h5><p className="environment-hint">{t('migrationHint')}</p>
      {spec.handoff && <><h5>{t('handoff')}</h5><p className="environment-request-handoff">{spec.handoff}</p></>}
      <div className="environment-request-digest"><span>{t('digest')}</span><code>{request.digest}</code><CopyButton value={request.digest} label={t('copyDigest')} /></div>
      <p className="environment-hint">{t('owner')}</p>
      {request.lastError && <ErrorMark message={t('failed')} detail={request.lastError} announce={false} />}
      <div className="environment-actions">
        {request.state === 'pending' && <><button type="button" className="host-secondary-button" disabled={busy || !canManage} onClick={() => void act('reject')}>{t('reject')}</button><button type="button" className="relay-button-primary" disabled={busy || !canManage} onClick={() => void act('approve')}>{t(busy ? 'working' : 'approve')}</button></>}
        {request.state === 'failed' && <button type="button" className="host-secondary-button" disabled={busy || !canManage} onClick={() => void act('retry')}>{t(busy ? 'working' : 'retry')}</button>}
        {(request.state === 'failed' || request.state === 'ready') && <button type="button" className="environment-danger" disabled={busy || !canManage} onClick={() => setRecover(true)}>{t('recoverHost')}</button>}
        {error && <ErrorMark message={error} detail={t('owner')} />}
      </div>
    </div>}
    <EnvironmentConfirmation open={recover} title={t('recoveryTitle')} description={t('recoveryBody')} confirmLabel={t('recoveryConfirm')} busy={busy} busyLabel={t('working')} error={error} onCancel={() => { if (!busy) setRecover(false); }} onConfirm={() => void act('recoverHost')} />
  </article>;
}
