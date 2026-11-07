import { useState } from 'react';
import { Plus, RefreshCw } from 'lucide-react';
import { useLocation } from 'react-router-dom';
import { ErrorMark } from '@pockymoe/thread-ui';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import { relayModeActive } from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { testEnvironmentConnection, type EnvironmentConnectionDto, type EnvironmentDto } from '../lib/environmentApi';
import { environmentText as t } from '../lib/environmentI18n';
import { EnvironmentRequests } from './environments/EnvironmentRequests';
import { ConnectionEditor } from './environments/ConnectionEditor';
import { BudgetEditor } from './environments/BudgetEditor';
import { EnvironmentCard } from './environments/EnvironmentCard';
import { environmentError, gibValue, useEnvironmentSnapshot } from './environments/model';
import { SectionHeading, Tag } from './environments/ui';
import './environments/environments.css';

export function DokploySettings() {
  useI18n();
  const { pathname } = useLocation();
  const deviceId = relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) return <p className="env-empty">{t('chooseDevice')}</p>;
  return <DeviceEnvironmentSettings key={deviceId ?? 'local'} deviceId={deviceId} />;
}

/** Destroyed environments stay listed (their records are retained) after the live ones. */
const byLiveness = (a: EnvironmentDto, b: EnvironmentDto) => Number(a.state === 'destroyed') - Number(b.state === 'destroyed') || b.updatedAt.localeCompare(a.updatedAt);

function DeviceEnvironmentSettings({ deviceId }: { deviceId: string | null }) {
  useI18n();
  const { snapshot, setSnapshot, loading, error, refresh } = useEnvironmentSnapshot(deviceId);
  const [editing, setEditing] = useState<EnvironmentConnectionDto | 'new' | null>(null);
  const [editBudget, setEditBudget] = useState(false);
  const [saved, setSaved] = useState(false);
  const environments = snapshot?.environments.slice().sort(byLiveness) ?? [];
  const running = environments.filter(env => env.state === 'running' || env.state === 'starting').length;
  const budget = snapshot?.budget;
  return <div className="env-settings" data-testid="dokploy-settings">
    {!snapshot && <div className="env-toolbar">
      {loading && <span role="status" className="env-dim">{t('loading')}</span>}
      {error && <ErrorMark message={error} onRetry={() => void refresh()} kind={error === t('unsupported') || error === t('forbidden') ? 'unavailable' : 'error'} />}
    </div>}
    <EnvironmentRequests deviceId={deviceId} hideWhenEmpty onChanged={() => void refresh()} />
    {snapshot && budget && <>
      <section className="env-section" aria-label={t('connections')}>
        <SectionHeading title={t('connections')} count={snapshot.connections.length}>
          {saved && <span role="status" className="env-ok">{t('saved')}</span>}
          {error && <ErrorMark message={error} onRetry={() => void refresh()} kind={error === t('unsupported') || error === t('forbidden') ? 'unavailable' : 'error'} />}
          <button type="button" className="env-icon-button" aria-label={t('refresh')} title={t('refresh')} disabled={loading} onClick={() => void refresh()}><RefreshCw aria-hidden="true" className={loading ? 'animate-spin' : undefined} /></button>
          <button type="button" className="env-button" onClick={() => setEditing('new')}><Plus aria-hidden="true" />{t('addConnection')}</button>
        </SectionHeading>
        {!snapshot.connections.length ? <p className="env-empty">{t('noConnections')}</p>
          : <div className="env-group env-list">{snapshot.connections.map(connection => <ConnectionRow key={connection.id} connection={connection} deviceId={deviceId} onEdit={() => setEditing(connection)} />)}</div>}
      </section>
      <section className="env-section" aria-label={t('budget')}>
        <SectionHeading title={t('budget')}>
          <button type="button" className="env-button" onClick={() => setEditBudget(true)}>{t('editBudget')}</button>
        </SectionHeading>
        <div className="env-group">
          <dl className="env-stats">
            <div><dt>{t('concurrent')}</dt><dd><b>{formatNumber(running)}</b><span> / {formatNumber(budget.maxRunning)}</span><small>{t('runningNow')}</small></dd></div>
            <div><dt>{t('cpuShort')}</dt><dd><b>{t('coresUnit', { n: formatNumber(budget.maxCpu) })}</b><small>{t('reserved')} {t('coresUnit', { n: formatNumber(budget.reservedCpu) })}</small></dd></div>
            <div><dt>{t('memoryShort')}</dt><dd><b>{t('gibUnit', { n: formatNumber(gibValue(budget.maxMemoryBytes)) })}</b><small>{t('reserved')} {t('gibUnit', { n: formatNumber(gibValue(budget.reservedMemoryBytes)) })}</small></dd></div>
          </dl>
          <p className="env-group-note">{t('budgetHint')}</p>
        </div>
      </section>
      <section className="env-section" aria-label={t('environments')}>
        <SectionHeading title={t('environments')} count={environments.filter(env => env.state !== 'destroyed').length} />
        {!environments.length ? <p className="env-empty">{t('noEnvironments')}</p>
          : <div className="env-group env-list">{environments.map(env => <EnvironmentCard key={env.id} environment={env} deviceId={deviceId}
            connection={snapshot.connections.find(connection => connection.id === env.connectionId)}
            onUpdate={next => setSnapshot(current => current && { ...current, environments: current.environments.map(value => value.id === next.id ? next : value) })} />)}</div>}
      </section>
    </>}
    {editing && <ConnectionEditor connection={editing === 'new' ? null : editing} deviceId={deviceId} onClose={() => setEditing(null)} onSaved={next => {
      setSnapshot(current => current && { ...current, connections: [...current.connections.filter(value => value.id !== next.id), next] }); setEditing(null); setSaved(true);
    }} />}
    {editBudget && budget && <BudgetEditor budget={budget} deviceId={deviceId} onClose={() => setEditBudget(false)} onSaved={next => {
      setSnapshot(current => current && { ...current, budget: next }); setEditBudget(false); setSaved(true);
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
  const egress = connection.allowedEgressHosts?.length ?? 0;
  return <article className="env-item" aria-label={connection.name}>
    <div className="env-item-head">
      <span className="env-row-main">
        <span className="env-row-title">{connection.name}</span>
        <span className="env-row-meta"><span>{connection.baseUrl}</span><code>{connection.image}</code>
          <span>{t('rootsCount', { n: formatNumber(connection.allowedRoots.length) })}</span>
          <span>{egress ? t('egressCount', { n: formatNumber(egress) }) : t('noEgress')}</span></span>
      </span>
      <Tag tone={connection.configured ? 'success' : 'warning'}>{t(connection.configured ? 'configured' : 'unconfigured')}</Tag>
      <div className="env-lifecycle">
        {success && <span role="status" className="env-ok">{t('tested')}</span>}
        {error && <ErrorMark message={error} onRetry={() => void test()} />}
        <button type="button" className="env-button" onClick={() => void test()} disabled={busy || !connection.configured}>{t(busy ? 'testing' : 'test')}</button>
        {!connection.configured && <ErrorMark kind="unavailable" message={t('unconfigured')} />}
        <button type="button" className="env-button" onClick={onEdit} disabled={busy}>{t('edit')}</button>
      </div>
    </div>
  </article>;
}
