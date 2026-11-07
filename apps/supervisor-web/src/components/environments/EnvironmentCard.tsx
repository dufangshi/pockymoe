import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { ErrorMark, PathChip, getDirtyWorkspaceDocumentCount } from '@pockymoe/thread-ui';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import { actOnEnvironment, notifyEnvironmentChanged, type EnvironmentAction, type EnvironmentDto, type EnvironmentLimits } from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';
import { GIB, environmentError, environmentTransitioning } from './model';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';

export function Limits({ value }: { value: EnvironmentLimits }) {
  return <dl className="environment-stats">
    <div><dt>{t('cpu')}</dt><dd>{formatNumber(value.cpu)}</dd></div>
    <div><dt>{t('memory')}</dt><dd>{formatNumber(value.memoryBytes / GIB)}</dd></div>
    <div><dt>{t('pids')}</dt><dd>{formatNumber(value.pids)}</dd></div>
    <div><dt>{t('nofile')}</dt><dd>{formatNumber(value.nofile)}</dd></div>
  </dl>;
}
export function EnvironmentCard({ environment: env, deviceId, onUpdate, disabledReason }: {
  environment: EnvironmentDto; deviceId?: string | null | undefined; onUpdate(env: EnvironmentDto): void; disabledReason?: string | undefined;
}) {
  useI18n();
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState<EnvironmentAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<'rebuild' | 'destroy' | null>(null);
  const act = async (action: EnvironmentAction) => {
    if (busy || disabledReason) return;
    if ((action === 'rebuild' || action === 'destroy') && getDirtyWorkspaceDocumentCount()) { setError(t('saveFilesFirst')); return; }
    setBusy(action); setError(null);
    try { const next = await actOnEnvironment(env.id, action, deviceId); onUpdate(next); setConfirmation(null); if (next.generation !== env.generation || action === 'destroy') notifyEnvironmentChanged(env.threadId, deviceId); }
    catch (cause) { setError(environmentError(cause)); }
    finally { setBusy(null); }
  };
  const transitioning = environmentTransitioning(env.state);
  const actions: EnvironmentAction[] = env.state === 'destroyed' ? [] : env.state === 'running' ? ['stop', 'rebuild', 'destroy']
    : transitioning ? [] : ['start', 'rebuild', 'destroy'];
  const state = (['creating', 'stopped', 'starting', 'running', 'stopping', 'error', 'destroyed', 'interrupted'] as string[]).includes(env.state) ? env.state as EnvironmentMessage : 'unknown';
  return <article className="environment-card" aria-label={env.name}>
    <div className="environment-card-heading">
      <button type="button" className="environment-expand" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>
        <ChevronRight aria-hidden="true" className={expanded ? 'is-expanded' : ''} />
        <span>{env.name}</span><span className="environment-state" data-state={env.state}>{t(state)}</span>
      </button>
      {env.lastError && <ErrorMark message={t('safeError')} detail={env.lastError} announce={false} />}
    </div>
    {expanded && <div className="environment-card-details">
      <p className="environment-hint">{t('generation')} {formatNumber(env.generation)}</p>
      <div className="environment-path"><span>{t('workspace')}</span><PathChip path={env.workspacePath} label={t('copyAbsolutePath')} /></div>
      {env.worktreePath && <div className="environment-path"><span>{t('worktree')}</span><PathChip path={env.worktreePath} label={t('copyAbsolutePath')} /></div>}
      <h5>{t('requestedLimits')}</h5><Limits value={env.limits} />
      <h5>{t('observedLimits')}</h5>{env.observedLimits ? <Limits value={env.observedLimits} /> : <p className="environment-hint">{t('notObserved')}</p>}
      <p className="environment-hint">{t('diskQuota')}</p>
    </div>}
    {!!actions.length && <div className="environment-actions">
      {actions.map(action => <button key={action} type="button" className={action === 'destroy' ? 'environment-danger' : 'host-secondary-button'}
        disabled={!!busy || !!disabledReason} onClick={() => action === 'rebuild' || action === 'destroy' ? setConfirmation(action) : void act(action)}>
        {busy === action ? t('working') : t(action)}
      </button>)}
      {disabledReason && <ErrorMark kind="unavailable" message={disabledReason} />}
      {error && !confirmation && <ErrorMark message={error} />}
    </div>}
    <EnvironmentConfirmation open={confirmation !== null} title={t(confirmation === 'destroy' ? 'destroyTitle' : 'rebuildTitle')}
      description={t(confirmation === 'destroy' ? 'destroyBody' : 'rebuildBody')} confirmLabel={t(confirmation ?? 'destroy')}
      busy={!!busy} busyLabel={t('working')} error={error}
      onCancel={() => { if (!busy) { setConfirmation(null); setError(null); } }} onConfirm={() => { if (confirmation) void act(confirmation); }} />
  </article>;
}
