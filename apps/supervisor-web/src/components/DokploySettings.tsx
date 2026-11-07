import { useState } from 'react';
import { useLocation } from 'react-router-dom';
import { ErrorMark } from '@pockymoe/thread-ui';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import { relayModeActive } from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { testEnvironmentConnection, type EnvironmentConnectionDto } from '../lib/environmentApi';
import { environmentText as t } from '../lib/environmentI18n';
import { ConnectionEditor } from './environments/ConnectionEditor';
import { BudgetEditor } from './environments/BudgetEditor';
import { EnvironmentCard } from './environments/EnvironmentCard';
import { GIB, environmentError, useEnvironmentSnapshot } from './environments/model';
import './environments/environments.css';

export function DokploySettings() {
  useI18n();
  const { pathname } = useLocation();
  const deviceId = relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) return <p className="environment-hint">{t('chooseDevice')}</p>;
  return <DeviceEnvironmentSettings key={deviceId ?? 'local'} deviceId={deviceId} />;
}
function DeviceEnvironmentSettings({ deviceId }: { deviceId: string | null }) {
  useI18n();
  const { snapshot, setSnapshot, loading, error, refresh } = useEnvironmentSnapshot(deviceId);
  const [editing, setEditing] = useState<EnvironmentConnectionDto | 'new' | null>(null);
  const [editBudget, setEditBudget] = useState(false);
  const [saved, setSaved] = useState(false);
  return <div className="environment-settings" data-testid="dokploy-settings">
    <div className="environment-toolbar"><span role="status" className="environment-hint">{loading ? t('loading') : saved ? t('saved') : ''}</span>
      <div className="environment-actions"><button type="button" className="host-secondary-button" disabled={loading} onClick={() => void refresh()}>{t('refresh')}</button>
        {error && <ErrorMark message={error} onRetry={() => void refresh()} kind={error === t('unsupported') || error === t('forbidden') ? 'unavailable' : 'error'} />}</div>
    </div>
    {snapshot && <>
      <section className="environment-section" aria-label={t('connections')}><header><h4>{t('connections')}</h4><button type="button" className="host-secondary-button" onClick={() => setEditing('new')}>{t('addConnection')}</button></header>
        {!snapshot.connections.length && <p className="environment-hint">{t('noConnections')}</p>}
        {snapshot.connections.map(connection => <ConnectionRow key={connection.id} connection={connection} deviceId={deviceId} onEdit={() => setEditing(connection)} />)}
      </section>
      <section className="environment-section" aria-label={t('budget')}><header><h4>{t('budget')}</h4><button type="button" className="host-secondary-button" onClick={() => setEditBudget(true)}>{t('editBudget')}</button></header>
        <p className="environment-hint">{t('budgetHint')}</p>
        <dl className="environment-stats environment-card">
          {([['maxRunning', snapshot.budget.maxRunning], ['maxCpu', snapshot.budget.maxCpu], ['maxMemory', snapshot.budget.maxMemoryBytes / GIB],
            ['reservedCpu', snapshot.budget.reservedCpu], ['reservedMemory', snapshot.budget.reservedMemoryBytes / GIB]] as const).map(([key, value]) => <div key={key}><dt>{t(key)}</dt><dd>{formatNumber(value)}</dd></div>)}
        </dl>
      </section>
      <section className="environment-section" aria-label={t('environments')}><header><h4>{t('environments')}</h4></header>
        {!snapshot.environments.length && <p className="environment-hint">{t('noEnvironments')}</p>}
        {snapshot.environments.map(env => <EnvironmentCard key={env.id} environment={env} deviceId={deviceId}
          onUpdate={next => setSnapshot(current => current && { ...current, environments: current.environments.map(value => value.id === next.id ? next : value) })} />)}
      </section>
    </>}
    {editing && <ConnectionEditor connection={editing === 'new' ? null : editing} deviceId={deviceId} onClose={() => setEditing(null)} onSaved={next => {
      setSnapshot(current => current && { ...current, connections: [...current.connections.filter(value => value.id !== next.id), next] }); setEditing(null); setSaved(true);
    }} />}
    {editBudget && snapshot && <BudgetEditor budget={snapshot.budget} deviceId={deviceId} onClose={() => setEditBudget(false)} onSaved={budget => {
      setSnapshot(current => current && { ...current, budget }); setEditBudget(false); setSaved(true);
    }} />}
  </div>;
}
function ConnectionRow({ connection, deviceId, onEdit }: { connection: EnvironmentConnectionDto; deviceId: string | null; onEdit(): void }) {
  const [busy, setBusy] = useState(false);
  const [success, setSuccess] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const test = async () => {
    if (busy) return;
    setBusy(true); setSuccess(false); setError(null);
    try { const result = await testEnvironmentConnection(connection.id, deviceId); if (result.ok) setSuccess(true); else setError(t('testFailed')); }
    catch (cause) { setError(environmentError(cause, 'testFailed')); }
    finally { setBusy(false); }
  };
  return <article className="environment-card" aria-label={connection.name}>
    <div className="environment-card-heading"><div><strong>{connection.name}</strong><p className="environment-hint">{connection.baseUrl}</p></div><span className="environment-hint">{t(connection.configured ? 'configured' : 'unconfigured')}</span></div>
    <div className="environment-actions"><button type="button" className="host-secondary-button" onClick={onEdit} disabled={busy}>{t('edit')}</button>
      <button type="button" className="host-secondary-button" onClick={() => void test()} disabled={busy || !connection.configured}>{t(busy ? 'testing' : 'test')}</button>
      {!connection.configured && <ErrorMark kind="unavailable" message={t('unconfigured')} />}
      {success && <span role="status" className="environment-success">{t('tested')}</span>}
      {error && <ErrorMark message={error} onRetry={() => void test()} />}
    </div>
  </article>;
}
