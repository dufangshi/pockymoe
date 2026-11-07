import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { CopyButton, ErrorMark, PathChip, getDirtyWorkspaceDocumentCount } from '@pockymoe/thread-ui';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import { actOnEnvironment, notifyEnvironmentChanged, type EnvironmentAction, type EnvironmentConnectionDto, type EnvironmentDto } from '../../lib/environmentApi';
import { environmentText as t } from '../../lib/environmentI18n';
import { environmentRequestText as rt } from '../../lib/environmentRequestI18n';
import { environmentActions, environmentError } from './model';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';
import { EnvironmentStateMark, Fact, limitsText, sameLimits } from './ui';

/** Start/stop, rebuild and destroy for one environment. Rebuild and destroy
    confirm first (Cancel focused); destroy is set apart in the danger color. */
export function EnvironmentLifecycle({ environment: env, deviceId, onUpdate, disabledReason }: {
  environment: EnvironmentDto; deviceId?: string | null | undefined; onUpdate(env: EnvironmentDto): void; disabledReason?: string | undefined;
}) {
  useI18n();
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
  const actions = environmentActions(env.state);
  if (!actions.length) return null;
  return <div className="env-lifecycle">
    {actions.map(action => <button key={action} type="button" className={`env-button${action === 'destroy' ? ' is-danger' : ''}`}
      disabled={!!busy || !!disabledReason} onClick={() => action === 'rebuild' || action === 'destroy' ? setConfirmation(action) : void act(action)}>
      {busy === action ? t('working') : t(action)}
    </button>)}
    {disabledReason && <ErrorMark kind="unavailable" message={disabledReason} />}
    {error && !confirmation && <ErrorMark message={error} />}
    <EnvironmentConfirmation open={confirmation !== null} title={t(confirmation === 'destroy' ? 'destroyTitle' : 'rebuildTitle')}
      description={t(confirmation === 'destroy' ? 'destroyBody' : 'rebuildBody')} confirmLabel={t(confirmation ?? 'destroy')}
      busy={!!busy} busyLabel={t('working')} error={error}
      onCancel={() => { if (!busy) { setConfirmation(null); setError(null); } }} onConfirm={() => { if (confirmation) void act(confirmation); }} />
  </div>;
}

/** Where an environment's files come from and what limits it has. */
export function EnvironmentFacts({ environment: env, connection, showThread = false }: { environment: EnvironmentDto; connection?: EnvironmentConnectionDto | undefined; showThread?: boolean }) {
  const observed = env.observedLimits;
  return <dl className="env-facts is-compact">
    {connection && <Fact label={t('connection')} note={<code>{connection.image}</code>}>{connection.name} <span className="env-dim">· Dokploy · {t('generationN', { n: formatNumber(env.generation) })}</span></Fact>}
    {showThread && <Fact label={t('thread')}><span className="env-id"><code>{env.threadId}</code><CopyButton value={env.threadId} label={rt('copyThread')} /></span></Fact>}
    <Fact label={t('workspace')}><PathChip path={env.workspacePath} label={t('copyAbsolutePath')} /></Fact>
    {env.worktreePath && env.worktreePath !== env.workspacePath && <Fact label={t('worktree')}><PathChip path={env.worktreePath} label={t('copyAbsolutePath')} /></Fact>}
    <Fact label={t('resources')} note={t('diskQuota')}>{limitsText(env.limits)}</Fact>
    <Fact label={t('observedLimits')}>{!observed ? <span className="env-dim">{t('notObserved')}</span>
      : sameLimits(observed, env.limits) ? <span className="env-ok">{t('observedVerified')}</span> : limitsText(observed)}</Fact>
  </dl>;
}

/** Settings row: state, name and lifecycle on one line; paths and limits fold. */
export function EnvironmentCard({ environment: env, connection, deviceId, onUpdate, disabledReason }: {
  environment: EnvironmentDto; connection?: EnvironmentConnectionDto | undefined; deviceId?: string | null | undefined; onUpdate(env: EnvironmentDto): void; disabledReason?: string | undefined;
}) {
  useI18n();
  const [expanded, setExpanded] = useState(false);
  return <article className="env-item" aria-label={env.name} data-state={env.state}>
    <div className="env-item-head">
      <button type="button" className="env-expand" aria-expanded={expanded} aria-label={`${env.name} · ${t('environmentDetails')}`} onClick={() => setExpanded(value => !value)}>
        <ChevronRight aria-hidden="true" />
        <span className="env-row-main">
          <span className="env-row-title">{env.name}</span>
          <span className="env-row-meta"><span>{connection?.name ?? 'Dokploy'}</span><span>{limitsText(env.limits)}</span><code>{env.threadId.slice(0, 8)}</code></span>
        </span>
      </button>
      <EnvironmentStateMark state={env.state} />
      {env.lastError && <ErrorMark message={t('safeError')} detail={env.lastError} announce={false} />}
      <EnvironmentLifecycle environment={env} deviceId={deviceId} onUpdate={onUpdate} disabledReason={disabledReason} />
    </div>
    {expanded && <div className="env-item-details"><EnvironmentFacts environment={env} connection={connection} showThread /></div>}
  </article>;
}
