import { useEffect, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { Download, RefreshCw, RotateCw } from 'lucide-react';
import { ApiError, relayModeActive, request } from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { FormDialog } from './FormDialog';
import { UpstreamManagement } from './UpstreamManagement';

type Installation = {
  installed?: boolean;
  legacyInstall?: boolean;
  canInstall?: boolean;
  path: string;
  resolvedPath: string;
  version?: string;
  manager: string;
  canUpdate: boolean;
  updateCommand?: string;
  reason?: string;
};
type Job = {
  state?: string;
  phase?: string;
  action?: string;
  component?: string;
  connectionVerified?: boolean;
  rollingBack?: boolean;
  error?: string;
  targetVersion?: string;
};
type Harness = {
  id: string;
  name: string;
  transport?: string;
  base: Installation | null;
  adapter: Installation | null;
  job?: Job;
};
type Supervisor = {
  runningVersion?: string;
  installedVersion?: string;
  latestVersion?: string;
  canUpdate: boolean;
  canRestart?: boolean;
  startedAt?: string;
  uptimeSeconds?: number;
  observedAt?: number;
  reason?: string;
  job?: Job | undefined;
};
const active = (job?: Job) =>
  [
    'running',
    'scheduled',
    'preparing',
    'installing',
    'restarting',
    'verifying',
  ].includes(job?.state ?? job?.phase ?? '');
function supervisorJobText(job: Job) {
  const operation = job.action === 'restart' ? 'Restart' : 'Update';
  if (job.rollingBack && active(job))
    return `${operation} failed; restoring the previous service…`;
  switch (job.phase) {
    case 'scheduled':
      return `${operation} requested…`;
    case 'preparing':
      return `Preparing ${operation.toLowerCase()}: saving active tasks…`;
    case 'installing':
      return 'Installing Supervisor update…';
    case 'restarting':
      return 'Restarting Supervisor…';
    case 'verifying':
      return 'Supervisor started; verifying its connection…';
    case 'completed':
      return `${operation} completed`;
    case 'recovered':
      return 'Supervisor recovered';
    case 'failed':
      return `Last ${operation.toLowerCase()} failed: ${job.error ?? 'Check the device logs.'}`;
    case 'rolled-back':
      return `Last ${operation.toLowerCase()} failed; previous service restored. ${job.error ?? ''}`;
    case 'rollback-failed':
      return `Last ${operation.toLowerCase()} and recovery failed: ${job.error ?? 'Check the device logs.'}`;
    default:
      return job.error ?? job.phase;
  }
}
const button =
  'host-secondary-button inline-flex min-h-9 items-center justify-center gap-1.5 rounded-md border px-2.5 text-xs disabled:opacity-50';
export function RuntimeManagement({
  view = 'all',
  deviceId: explicitDeviceId,
}: {
  view?: 'all' | 'device' | 'harnesses';
  deviceId?: string;
}) {
  const { pathname } = useLocation();
  const deviceId = explicitDeviceId ?? relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) {
    return (
      <section className="py-5" aria-label="Runtime management">
        <h3 className="text-sm font-semibold">Device runtimes</h3>
        <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
          Open a device to view and manage its Supervisor and harness versions.
        </p>
      </section>
    );
  }
  // Route identity owns both the state and requests. Never fall back to a
  // remembered device, or retain another device's open update confirmation.
  return (
    <DeviceRuntimeManagement
      view={view}
      key={deviceId ?? 'local'}
      apiRoot={
        deviceId ? `/relay/devices/${encodeURIComponent(deviceId)}/api` : '/api'
      }
    />
  );
}

function DeviceRuntimeManagement({
  apiRoot,
  view,
}: {
  apiRoot: string;
  view: 'all' | 'device' | 'harnesses';
}) {
  const [selectedHarness, setSelectedHarness] = useState('codex');
  const api = <T,>(path: string, action?: unknown) =>
    request<T>(
      `${apiRoot}/management/${path}`,
      action
        ? { method: 'POST', body: JSON.stringify(action) }
        : { cache: 'no-store' },
    );
  const [supervisor, setSupervisor] = useState<Supervisor | null>(null);
  const [harnesses, setHarnesses] = useState<Harness[]>([]);
  const [jobs, setJobs] = useState<Record<string, Job>>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [reconnectSince, setReconnectSince] = useState<number | null>(null);
  const [ownerDenied, setOwnerDenied] = useState(false);
  const [clock, setClock] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setClock(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const [confirm, setConfirm] = useState<{
    id?: string;
    action?: 'restart';
    installing?: boolean;
    legacyInstall?: boolean;
    component?: string;
    name: string;
    command?: string | undefined;
  } | null>(null);
  async function load() {
    try {
      const [s, h] = await Promise.all([
        api<Supervisor>('supervisor'),
        api<Harness[]>('harnesses'),
      ]);
      setSupervisor((previous) => ({
        ...previous,
        ...s,
        job: s.job,
        observedAt: performance.now(),
      }));
      setHarnesses(h);
      setJobs(
        Object.fromEntries(
          h.filter((row) => row.job).map((row) => [row.id, row.job!]),
        ),
      );
    } catch (error) {
      if (error instanceof ApiError && error.statusCode === 403) {
        setOwnerDenied(true);
        setSupervisor(null);
        setHarnesses([]);
        return;
      }
      // Older devices may serve their SPA fallback for an unknown management route.
      if (
        !(error instanceof SyntaxError) &&
        !(error instanceof ApiError && error.statusCode === 404)
      )
        throw error;
      const current = await request<{ version: string }>(`${apiRoot}/version`);
      setSupervisor({
        runningVersion: current.version,
        canUpdate: false,
        reason:
          'Upgrade this device to 0.12.17 or later with its existing installer to enable runtime management.',
      });
      setHarnesses([]);
    }
  }
  useEffect(() => {
    void load().catch((e) => setError(e.message));
  }, []);
  const pending = Object.values(jobs).some(active) || active(supervisor?.job);
  useEffect(() => {
    if (!pending) return;
    let stopped = false;
    let loading = false;
    const timer = window.setInterval(async () => {
      if (loading) return;
      loading = true;
      try {
        const [j, s] = await Promise.all([
          api<Record<string, Job>>('jobs'),
          api<Supervisor>('supervisor'),
        ]);
        if (stopped) return;
        setReconnectSince(null);
        setJobs(j);
        setSupervisor((previous) => ({
          ...previous,
          ...s,
          job: s.job,
          observedAt: performance.now(),
        }));
        if (!Object.values(j).some(active) && !active(s.job)) await load();
      } catch {
        /* Temporary disconnect during supervisor replacement; keep polling. */
        const lostAt = Date.now();
        if (!stopped) setReconnectSince((previous) => previous ?? lostAt);
      } finally {
        loading = false;
      }
    }, 2500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [pending]);
  async function act(id: string, action: string, component = 'base') {
    setBusy(true);
    setError('');
    try {
      await api(`harnesses/${encodeURIComponent(id)}`, { action, component });
      setJobs((prev) => ({ ...prev, [id]: { state: 'running', action } }));
      setConfirm(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Unable to manage harness');
    } finally {
      setBusy(false);
    }
  }
  async function supervisorAction(action: 'check' | 'update' | 'restart') {
    setBusy(true);
    setError('');
    try {
      const next = await api<Supervisor>(`supervisor/${action}`, {});
      setSupervisor((previous) => ({
        ...previous,
        ...next,
        observedAt: performance.now(),
      }));
      if (
        (action === 'update' && next.canUpdate) ||
        (action === 'restart' && next.canRestart)
      )
        setConfirm(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Unable to update Supervisor');
    } finally {
      setBusy(false);
    }
  }
  async function installLegacyAdapter(id: string) {
    setBusy(true);
    setError('');
    try {
      await request(
        `${apiRoot}/agent-runtimes/acp/install?agentId=${encodeURIComponent(id)}`,
        {
          method: 'POST',
          body: JSON.stringify({}),
        },
      );
      setConfirm(null);
      await load();
    } catch (error) {
      setError(
        `${error instanceof Error ? error.message : 'Adapter installation failed'}. Older Supervisors install through the system npm prefix; update the Supervisor to use managed adapter installation.`,
      );
    } finally {
      setBusy(false);
    }
  }
  function installation(row: Harness, data: Installation, component: string) {
    return (
      <div className="mt-2 min-w-0 text-xs text-[var(--theme-fg-muted)]">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span>
            {component === 'adapter' ? 'ACP adapter · ' : ''}
            {data.installed === false
              ? 'Not installed'
              : `${data.version ?? 'Version unavailable'} · ${data.manager ?? 'managed'}`}
          </span>
          {(data.canUpdate || data.canInstall) && (
            <button
              className={button}
              disabled={busy || active(jobs[row.id])}
              aria-label={`${data.installed === false ? 'Install' : 'Update'} ${row.name}${component === 'adapter' ? ' adapter' : ''}`}
              onClick={() =>
                setConfirm({
                  id: row.id,
                  name: row.name,
                  component,
                  installing: data.installed === false,
                  legacyInstall: data.legacyInstall ?? false,
                  command: data.updateCommand,
                })
              }
            >
              <Download size={13} />
              {data.installed === false ? 'Install' : 'Update'}
            </button>
          )}
        </div>
        <details className="mt-2">
          <summary className="cursor-pointer text-[11px]">
            Installation details
          </summary>
          <p
            className="mt-1 break-all font-mono text-[11px]"
            title={data.resolvedPath}
          >
            {data.path}
          </p>
        </details>
        {data.reason && <p className="mt-1 leading-5">{data.reason}</p>}
      </div>
    );
  }
  function row(h: Harness) {
    const job = jobs[h.id];
    return (
      <div
        key={h.id}
        className="min-w-0 border-t border-[var(--theme-border)] py-3"
      >
        <div className="flex items-center justify-between gap-2">
          <h4 className="text-sm font-medium">{h.name}</h4>
          <button
            className={button}
            disabled={
              busy || !h.base || h.base.installed === false || active(job)
            }
            title="Reload this harness's configuration. Other harnesses stay connected."
            aria-label={`Restart ${h.name}`}
            onClick={() => void act(h.id, 'restart')}
          >
            <RotateCw size={13} />
            Restart
          </button>
        </div>
        {h.base ? (
          installation(h, h.base, 'base')
        ) : (
          <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
            Not installed
          </p>
        )}
        {(h.adapter || h.transport === 'adapter') && (
          <details className="settings-detail">
            <summary className="cursor-pointer text-xs text-[var(--theme-fg-muted)]">
              ACP adapter
              {!h.adapter || h.adapter.installed === false
                ? ' · Installation required'
                : ' · Installed'}
            </summary>
            <div>
              {installation(
                h,
                h.adapter ?? {
                  installed: false,
                  canInstall: true,
                  legacyInstall: true,
                  path: '',
                  resolvedPath: '',
                  manager: '',
                  canUpdate: false,
                  reason:
                    'Adapter not detected. This older Supervisor uses its existing npm installer; update the Supervisor for user-owned dependency management.',
                },
                'adapter',
              )}
            </div>
          </details>
        )}
        {job && (
          <p
            role={job.error ? 'alert' : 'status'}
            className={`mt-2 text-xs ${job.error ? 'text-[var(--status-danger-fg)]' : 'text-[var(--theme-fg-muted)]'}`}
          >
            {job.error ??
              (active(job)
                ? `${job.action === 'install' ? 'Installing' : job.action === 'update' ? 'Updating' : 'Restarting'}…`
                : job.connectionVerified
                  ? 'ACP connection verified'
                  : `${job.action === 'update' && job.component === 'base' ? 'Base component update completed' : 'Completed'} · configuration reloads on the next turn`)}
          </p>
        )}
      </div>
    );
  }
  if (ownerDenied)
    return (
      <section className="py-5" aria-label="Runtime management">
        <h3 className="text-sm font-semibold">Supervisor</h3>
        <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
          Only the device owner can manage or restart this Supervisor.
        </p>
      </section>
    );
  const seconds =
    supervisor?.uptimeSeconds !== undefined
      ? supervisor.uptimeSeconds +
        Math.max(
          0,
          Math.floor(
            (performance.now() - (supervisor.observedAt ?? performance.now())) /
              1000,
          ),
        )
      : supervisor?.startedAt
        ? Math.max(
            0,
            Math.floor((clock - Date.parse(supervisor.startedAt)) / 1000),
          )
        : undefined;
  const uptime =
    seconds === undefined
      ? null
      : `${Math.floor(seconds / 86400)}d ${Math.floor(seconds / 3600) % 24}h ${Math.floor(seconds / 60) % 60}m ${seconds % 60}s`;
  return (
    <section className="py-1" aria-label="Runtime management">
      {view !== 'harnesses' && (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold">Supervisor</h3>
              <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                {supervisor?.runningVersion
                  ? `${active(supervisor.job) ? 'Version' : 'Running'} ${supervisor.runningVersion}`
                  : 'Loading version…'}
                {supervisor?.latestVersion &&
                  ` · Latest ${supervisor.latestVersion}`}
              </p>
            </div>
            <div className="flex gap-2">
              {supervisor?.canRestart && (
                <button
                  className={button}
                  disabled={busy || active(supervisor.job)}
                  onClick={() =>
                    setConfirm({ name: 'Supervisor', action: 'restart' })
                  }
                >
                  <RotateCw size={13} /> Restart Supervisor
                </button>
              )}
              <button
                className={button}
                disabled={
                  busy || !supervisor?.canUpdate || active(supervisor?.job)
                }
                onClick={() => void supervisorAction('check')}
              >
                <RefreshCw size={13} />
                Check updates
              </button>
              {supervisor?.latestVersion &&
                (supervisor.latestVersion !== supervisor.runningVersion ||
                  (supervisor.installedVersion &&
                    supervisor.latestVersion !==
                      supervisor.installedVersion)) &&
                supervisor.canUpdate && (
                  <button
                    className={button}
                    disabled={busy || active(supervisor.job)}
                    onClick={() => setConfirm({ name: 'Supervisor' })}
                  >
                    <Download size={13} />
                    Update
                  </button>
                )}
            </div>
          </div>
          {uptime && (
            <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
              Uptime {uptime}
            </p>
          )}
          {supervisor?.installedVersion &&
            supervisor.installedVersion !== supervisor.runningVersion && (
              <p className="mt-2 text-xs">
                Installed {supervisor.installedVersion}; running{' '}
                {supervisor.runningVersion}. Check updates to bring the
                installation and running service to the same version.
              </p>
            )}
          {supervisor?.reason && (
            <p
              role="status"
              className="mt-2 text-xs text-[var(--theme-fg-muted)]"
            >
              {supervisor.reason}
            </p>
          )}
          {supervisor?.job && (
            <p role="status" className="mt-2 text-xs">
              {reconnectSince
                ? 'Waiting for the device to reconnect…'
                : supervisorJobText(supervisor.job)}
              {reconnectSince && clock - reconnectSince > 30_000 && (
                <span className="mt-1 block text-[var(--status-warning-fg)]">
                  The device has not returned yet. Installation may have
                  finished, but restart has not been verified. Check the
                  Supervisor update and launch logs on the device. This page
                  will keep checking automatically.
                </span>
              )}
              {active(supervisor.job) && (
                <span className="mt-1 block text-[var(--theme-fg-muted)]">
                  Controls are temporarily disabled until this operation
                  finishes.
                </span>
              )}
            </p>
          )}
        </>
      )}
      {view !== 'device' && (
        <>
          <div
            className="flex flex-wrap gap-2 pb-4"
            role="group"
            aria-label="Choose harness"
          >
            {[
              ...new Map(
                [
                  ...[
                    { id: 'codex', name: 'Codex' },
                    { id: 'claude', name: 'Claude Code' },
                    { id: 'gemini', name: 'Gemini CLI' },
                    { id: 'grok', name: 'Grok Build' },
                  ],
                  ...harnesses,
                ].map((h) => [h.id, h]),
              ).values(),
            ].map((h) => (
              <button
                key={h.id}
                type="button"
                aria-pressed={selectedHarness === h.id}
                className={`min-h-10 rounded-xl border px-3 text-xs transition ${selectedHarness === h.id ? 'border-[var(--theme-accent-border)] bg-[var(--theme-accent-soft)] text-[var(--theme-accent-strong)]' : 'border-[var(--theme-border)] hover:bg-[var(--theme-hover)]'}`}
                onClick={() => setSelectedHarness(h.id)}
              >
                {h.name}
              </button>
            ))}
          </div>
          {harnesses.filter((h) => h.id === selectedHarness).map(row)}
          {['codex', 'claude', 'gemini', 'grok'].includes(selectedHarness) ? (
            <UpstreamManagement
              key={apiRoot + selectedHarness}
              apiRoot={apiRoot}
              harness={selectedHarness}
            />
          ) : (
            <p className="mt-4 rounded-xl border border-dashed border-[var(--theme-border)] p-4 text-xs text-[var(--theme-fg-muted)]">
              Use this harness's native configuration to manage upstreams.
            </p>
          )}
        </>
      )}
      {view !== 'harnesses' && (
        <UpstreamManagement
          key={apiRoot + 'templates'}
          apiRoot={apiRoot}
          templatesOnly
        />
      )}

      {error && (
        <p role="alert" className="mt-3 text-xs text-[var(--status-danger-fg)]">
          {error}
        </p>
      )}
      {confirm && (
        <FormDialog
          title={`${confirm.action === 'restart' ? 'Restart' : confirm.installing ? 'Install' : 'Update'} ${confirm.name}`}
          busy={busy}
          onClose={() => setConfirm(null)}
          description={
            confirm.action === 'restart'
              ? 'The device will briefly disconnect. Running tasks will be paused and continued in their existing sessions after restart; queued messages will be preserved.'
              : confirm.id
                ? confirm.installing
                  ? 'Install this component on the selected device. Required ACP adapters are included with a harness installation.'
                  : 'Update the selected installation, then reload its configuration. Running turns must finish first.'
                : 'The Supervisor will briefly disconnect. An independent system job will install, verify and restart it, and roll back if startup fails. Running tasks will be paused and continued after restart.'
          }
        >
          {confirm.command && (
            <code className="break-all text-xs">{confirm.command}</code>
          )}
          {error && (
            <p role="alert" className="text-sm text-[var(--status-danger-fg)]">
              {error}
            </p>
          )}
          <button
            className="relay-button-primary min-h-11"
            disabled={busy}
            onClick={() =>
              void (confirm.id
                ? confirm.legacyInstall
                  ? installLegacyAdapter(confirm.id)
                  : act(
                      confirm.id,
                      confirm.installing && confirm.component === 'base'
                        ? 'install'
                        : 'update',
                      confirm.component,
                    )
                : supervisorAction(confirm.action ?? 'update'))
            }
          >
            {busy
              ? 'Starting…'
              : confirm.action === 'restart'
                ? 'Restart'
                : confirm.installing
                  ? 'Install'
                  : 'Update'}
          </button>
        </FormDialog>
      )}
    </section>
  );
}
