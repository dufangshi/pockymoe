import { useCallback, useEffect, useState } from 'react';
import { Box } from 'lucide-react';
import { ErrorMark, Select, getDirtyWorkspaceDocumentCount } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import type { ThreadDto } from '@pockymoe/shared';
import { bindThreadEnvironment, createEnvironment, fetchThreadEnvironment, notifyEnvironmentChanged, type EnvironmentDto, type ThreadEnvironmentDto } from '../lib/environmentApi';
import { environmentText as t } from '../lib/environmentI18n';
import { EnvironmentWindow } from './environments/EnvironmentWindow';
import { EnvironmentCard } from './environments/EnvironmentCard';
import { EnvironmentConfirmation } from './environments/EnvironmentConfirmation';
import { environmentError, useEnvironmentSnapshot } from './environments/model';
import { EnvironmentRequests } from './environments/EnvironmentRequests';

type Props = {
  thread: Pick<ThreadDto, 'id' | 'status' | 'agentId' | 'provider' | 'providerSessionId' | 'updatedAt' | 'parentThreadId'>;
  deviceId?: string | null | undefined;
  canManage: boolean;
};
export function ThreadEnvironmentControl(props: Props) {
  useI18n();
  const [open, setOpen] = useState(false);
  return <>
    <button type="button" aria-label={t('title')} title={t('title')} onClick={() => setOpen(true)}><Box aria-hidden="true" /></button>
    {open && <ThreadEnvironmentDialog {...props} onClose={() => setOpen(false)} />}
  </>;
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
  const activeThread = thread.status === 'running' || thread.status === 'recovering';
  const inheritedEnvironment = !!(thread.parentThreadId && binding?.environment);
  // Custom ACP threads use the existing "acp" provider; the environment API names that family "custom".
  const environmentFamily = thread.provider === 'acp' ? 'custom' : thread.provider;
  const supported = !!binding?.supportedHarnesses.some(harness => harness === thread.agentId || harness === thread.provider || harness === environmentFamily);
  const lifecycleReason = !canManage ? t('readOnly') : activeThread ? t('threadBusy') : inheritedEnvironment ? t('inheritedEnvironment') : undefined;
  const reason = lifecycleReason ?? (!supported && binding ? t('unsupportedHarness') : undefined);
  const available = snapshot?.environments.filter(env => (env.threadId === thread.id || env.id === current?.id) && env.state !== 'destroyed') ?? [];
  if (current && current.state !== 'destroyed' && !available.some(env => env.id === current.id)) available.push(current);
  const connections = snapshot?.connections.filter(connection => connection.configured) ?? [];
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
  return <EnvironmentWindow title={t('title')} onClose={onClose} busy={busy} dirty={dirty}>
    <div className="environment-form" data-testid="thread-environment">
      <EnvironmentRequests threadId={thread.id} deviceId={deviceId} canManage={canManage} onChanged={() => { setRevision(value => value + 1); void refresh(); }} />
      <div className="environment-toolbar"><h3>{t('current')}</h3><button type="button" className="host-secondary-button" disabled={loading || bindingLoading || busy}
        onClick={() => { void refresh(); setRevision(value => value + 1); }}>{t('refresh')}</button></div>
      {(loading || bindingLoading) && <p role="status" className="environment-hint">{t('loading')}</p>}
      {(snapshotError || bindingError) && <div className="environment-actions"><button type="button" onClick={() => { void refresh(); setRevision(value => value + 1); }}>{t('retry')}</button><ErrorMark message={snapshotError || bindingError || ''} /></div>}
      {binding && <>
        {current ? <EnvironmentCard key={current.id} environment={current} deviceId={deviceId} onUpdate={updateEnvironment} {...(lifecycleReason ? { disabledReason: lifecycleReason } : {})} /> : <p className="environment-hint">{t('host')}</p>}
        <p className="environment-hint">{t('diskQuota')}</p>
        <p className="environment-hint">{t('isolationHint')}</p>
        {!!binding.limitations.length && <div className="environment-actions"><span className="environment-hint">{t('limitations')}</span><ErrorMark kind="unavailable" message={t('limitations')} detail={binding.limitations.join('\n')} announce={false} /></div>}
      </>}
      {binding && snapshot && !createMode && <>
        <label><span>{t('select')}</span><Select aria-label={t('select')} value={selected} disabled={busy || !!unavailable}
          onValueChange={value => { setSelected(value); setSaved(false); setError(null); }} options={[{ value: '', label: t('host') }, ...available.map(env => ({ value: env.id, label: env.name }))]} /></label>
        <div className="environment-actions"><button type="button" className="host-secondary-button" disabled={busy || !!unavailable || !connections.length}
          onClick={() => { setCreateMode(true); setSaved(false); setError(null); }}>{t('create')}</button>
          <button type="button" className="relay-button-primary" disabled={busy || !!unavailable || selected === (binding.environment?.id ?? '')} onClick={() => void apply()}>{t(busy ? 'saving' : 'apply')}</button>
          {reason && <ErrorMark kind="unavailable" message={reason} />}
          {!connections.length && <ErrorMark kind="unavailable" message={t('setupHint')} />}
          {error && <ErrorMark message={error} />}
          {saved && <span role="status" className="environment-success">{t('applied')}</span>}
        </div>
      </>}
      {createMode && <form className="environment-form" onSubmit={event => { event.preventDefault(); void create(); }}>
        <h3>{t('create')}</h3><p className="environment-hint">{t('createHint')}</p>
        <label><span>{t('connection')}</span><Select aria-label={t('connection')} value={chosenConnection} disabled={busy || !!unavailable} onValueChange={setConnectionId}
          options={connections.map(connection => ({ value: connection.id, label: connection.name }))} /></label>
        <label><span>{t('optionalName')}</span><input className="relay-input" value={name} disabled={busy} maxLength={120} onChange={event => setName(event.target.value)} /></label>
        <div className="environment-actions"><button type="button" className="host-secondary-button" disabled={busy} onClick={() => setCreateMode(false)}>{t('cancel')}</button>
          <button type="submit" className="relay-button-primary" disabled={busy || !!unavailable || !chosenConnection}>{t(busy ? 'creatingAndBinding' : 'createAndUse')}</button>
          {reason && <ErrorMark kind="unavailable" message={reason} />}{error && <ErrorMark message={error} />}
        </div>
      </form>}
      <EnvironmentConfirmation open={resetAction !== null} title={t('resetTitle')} description={t('resetBody')} confirmLabel={t('resetConfirm')} busy={busy} busyLabel={t('working')}
        error={error} onCancel={() => { if (!busy) setResetAction(null); }} onConfirm={() => { if (resetAction === 'create') void create(true); else void apply(true); }} />
    </div>
  </EnvironmentWindow>;
}
