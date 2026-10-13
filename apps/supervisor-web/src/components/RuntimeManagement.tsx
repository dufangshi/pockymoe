import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
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
  /** Command names; absent on devices before 0.12.81. */
  baseCommand?: string;
  adapterCommand?: string | null;
  base: Installation | null;
  adapter: Installation | null;
  job?: Job;
};
type HarnessStatus = 'ready' | 'adapter' | 'missing';
function harnessStatus(h: Harness): HarnessStatus {
  if (!h.base || h.base.installed === false) return 'missing';
  if (h.transport === 'adapter' && (!h.adapter || h.adapter.installed === false))
    return 'adapter';
  return 'ready';
}
const statusDot: Record<HarnessStatus, string> = {
  ready: 'bg-[var(--status-success-fg)]',
  adapter: 'bg-[var(--status-warning-fg)]',
  missing: 'bg-[var(--theme-fg-muted)] opacity-50',
};
const statusText = (status: HarnessStatus) =>
  status === 'ready'
    ? translate("devices.harnessReady")
    : status === 'adapter'
      ? translate("devices.harnessNeedsAdapter")
      : translate("devices.notInstalled");
function managerLabel(manager?: string) {
  switch (manager) {
    case 'pockymoe':
      return translate("devices.managedByPockymoe");
    case 'npm':
      return translate("devices.npmGlobalPackage");
    case 'homebrew':
      return 'Homebrew';
    case 'native':
      return translate("devices.selfUpdating");
    case 'app':
      return translate("devices.desktopApp");
    case 'manual':
      return translate("devices.installedManually");
    default:
      return manager;
  }
}
type Supervisor = {
  runningVersion?: string;
  installedVersion?: string;
  latestVersion?: string;
  canUpdate: boolean;
  canRestart?: boolean;
  /** Running from the retired npm package; Update moves it to the native runtime. */
  nativeMigration?: boolean;
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
  const operation = job.action === 'restart' ? translate("devices.restart_b134bd") : translate("devices.update");
  if (job.rollingBack && active(job))
    return translate("devices.failedRestoringThePreviousService", { value1: operation });
  switch (job.phase) {
    case 'scheduled':
      return `${operation} requested…`;
    case 'preparing':
      return `Preparing ${operation.toLowerCase()}: saving active tasks…`;
    case 'installing':
      return translate("devices.installingSupervisorUpdate");
    case 'restarting':
      return translate("devices.restartingSupervisor");
    case 'verifying':
      return translate("devices.supervisorStartedVerifyingItsConnection");
    case 'completed':
      return `${operation} completed`;
    case 'recovered':
      return translate("devices.supervisorRecovered");
    case 'failed':
      return `Last ${operation.toLowerCase()} failed: ${job.error ?? translate("devices.checkTheDeviceLogs")}`;
    case 'rolled-back':
      return `Last ${operation.toLowerCase()} failed; previous service restored. ${job.error ?? ''}`;
    case 'rollback-failed':
      return `Last ${operation.toLowerCase()} and recovery failed: ${job.error ?? translate("devices.checkTheDeviceLogs")}`;
    default:
      return job.error ?? job.phase;
  }
}
const button =
  'host-secondary-button inline-flex min-h-9 items-center justify-center gap-1.5 rounded-md border px-2.5 text-xs disabled:opacity-50';
export function RuntimeManagement({
  view = 'all',
}: {
  view?: 'all' | 'device' | 'harnesses';
}) {
  const { locale: i18nLocale } = useI18n();
  const { pathname } = useLocation();
  const deviceId = relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) {
    return (
      <section className="py-5" aria-label={translate("devices.runtimeManagement")}>
        <h3 className="text-sm font-semibold">{translate("devices.deviceRuntimes")}</h3>
        <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
          {translate("devices.openADeviceToViewAndManage")}</p>
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
        deviceId ? `relay/devices/${encodeURIComponent(deviceId)}/api` : 'api'
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
  const { locale: i18nLocale } = useI18n();
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
          translate("devices.upgradeThisDeviceTo01217"),
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
      setError(e instanceof Error ? e.message : translate("devices.unableToManageHarness"));
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
      setError(e instanceof Error ? e.message : translate("devices.unableToUpdateSupervisor"));
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
        translate("devices.olderSupervisorsInstallThroughTheSystemNpm", { value1: error instanceof Error ? error.message : translate("devices.adapterInstallationFailed") }),
      );
    } finally {
      setBusy(false);
    }
  }
  function installation(row: Harness, data: Installation, component: 'base' | 'adapter') {
    const missing = data.installed === false;
    const blocked = component === 'adapter' && harnessStatus(row) === 'missing';
    const command = component === 'base' ? row.baseCommand : row.adapterCommand;
    const source = managerLabel(data.manager);
    const hint = blocked
      ? translate("devices.installTheCommandLineToolFirst")
      : missing && component === 'base'
        ? [
            translate("devices.commandNotFoundOnPath", { value1: row.baseCommand ?? row.name }),
            data.reason,
          ]
            .filter(Boolean)
            .join(' ')
        : data.legacyInstall
          ? data.reason
          : !missing && !data.canUpdate
            ? data.manager === 'app'
              ? translate("devices.updateTheDesktopApp")
              : data.manager === 'manual'
                ? translate("devices.updateWithItsOwnInstaller")
                : data.reason
            : undefined;
    return (
      <li className="flex min-w-0 flex-wrap items-start justify-between gap-x-3 gap-y-2 py-3">
        <div className="min-w-0 flex-1 text-xs text-[var(--theme-fg-muted)]">
          <p className="font-medium text-[var(--theme-fg)]">
            {component === 'base'
              ? translate("devices.commandLineTool")
              : translate("devices.aCPAdapter_7d3feb")}
            {command && (
              <code className="ml-1.5 font-mono text-[11px] font-normal text-[var(--theme-fg-muted)]">
                {command}
              </code>
            )}
          </p>
          <p className="mt-0.5">
            {missing
              ? translate("devices.notInstalled")
              : [data.version ?? translate("devices.versionUnavailable"), source]
                  .filter(Boolean)
                  .join(' · ')}
          </p>
          {!missing && data.path && (
            <p
              className="mt-0.5 break-all font-mono text-[11px]"
              title={data.resolvedPath}
            >
              {data.path}
            </p>
          )}
          {hint && <p className="mt-1 leading-5">{hint}</p>}
        </div>
        {(data.canUpdate || data.canInstall) && (
          <button
            className={button}
            disabled={busy || active(jobs[row.id]) || blocked}
            aria-label={`${missing ? translate("devices.install") : translate("devices.update")} ${row.name}${component === 'adapter' ? ' adapter' : ''}`}
            onClick={() =>
              setConfirm({
                id: row.id,
                name: row.name,
                component,
                installing: missing,
                legacyInstall: data.legacyInstall ?? false,
                command: data.updateCommand,
              })
            }
          >
            <Download size={13} />
            {missing ? translate("devices.install") : translate("devices.update")}
          </button>
        )}
      </li>
    );
  }
  function row(h: Harness) {
    const job = jobs[h.id];
    const status = harnessStatus(h);
    return (
      <div
        key={h.id}
        className="settings-harness-row min-w-0 border-t border-[var(--theme-border)] py-3"
      >
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <h4 className="text-sm font-medium">{h.name}</h4>
            <p className="mt-0.5 flex items-center gap-1.5 text-xs text-[var(--theme-fg-muted)]">
              <span aria-hidden className={`h-2 w-2 shrink-0 rounded-full ${statusDot[status]}`} />
              {statusText(status)}
            </p>
          </div>
          {status !== 'missing' && (
            <button
              className={button}
              disabled={busy || active(job)}
              title={translate("devices.reloadThisHarnessSConfigurationOtherHarnesses")}
              aria-label={translate("devices.restart", { value1: h.name })}
              onClick={() => void act(h.id, 'restart')}
            >
              <RotateCw size={13} />
              {translate("devices.restart_b134bd")}</button>
          )}
        </div>
        <ul
          aria-label={translate("devices.harnessComponents", { value1: h.name })}
          className="mt-3 divide-y divide-[var(--theme-border)] rounded-lg border border-[var(--theme-border)] px-3"
        >
          {installation(
            h,
            h.base ?? {
              installed: false,
              path: '',
              resolvedPath: '',
              manager: '',
              canUpdate: false,
            },
            'base',
          )}
          {(h.adapter || h.transport === 'adapter') &&
            installation(
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
                  translate("devices.adapterNotDetectedThisOlderSupervisorUses"),
              },
              'adapter',
            )}
        </ul>
        {job && (
          <p
            role={job.error ? 'alert' : 'status'}
            className={`mt-2 text-xs ${job.error ? 'text-[var(--status-danger-fg)]' : 'text-[var(--theme-fg-muted)]'}`}
          >
            {job.error
              ? translate("devices.lastOperationFailed", { value1: job.error })
              : (active(job)
                ? `${job.action === 'install' ? translate("devices.installing") : job.action === 'update' ? translate("devices.updating") : translate("devices.restarting")}…`
                : job.connectionVerified
                  ? translate("devices.aCPConnectionVerified")
                  : translate("devices.configurationReloadsOnTheNextTurn", { value1: job.action === 'update' && job.component === 'base' ? translate("devices.baseComponentUpdateCompleted") : translate("devices.completed") }))}
          </p>
        )}
      </div>
    );
  }
  if (ownerDenied)
    return (
      <section className="py-5" aria-label={translate("devices.runtimeManagement")}>
        <h3 className="text-sm font-semibold">Supervisor</h3>
        <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
          {translate("devices.onlyTheDeviceOwnerCanManageOr")}</p>
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
    <section className="py-1" aria-label={translate("devices.runtimeManagement")}>
      {view !== 'harnesses' && (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold">Supervisor</h3>
              <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                {supervisor?.runningVersion
                  ? `${active(supervisor.job) ? translate("devices.version") : translate("devices.running_73989d")} ${supervisor.runningVersion}`
                  : translate("devices.loadingVersion")}
                {supervisor?.latestVersion &&
                  translate("devices.latest", { value1: supervisor.latestVersion })}
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
                  <RotateCw size={13} /> {translate("devices.restartSupervisor")}</button>
              )}
              <button
                className={button}
                disabled={
                  busy || !supervisor?.canUpdate || active(supervisor?.job)
                }
                onClick={() => void supervisorAction('check')}
              >
                <RefreshCw size={13} />
                {translate("devices.checkUpdates")}</button>
              {supervisor?.latestVersion &&
                (supervisor.latestVersion !== supervisor.runningVersion ||
                  supervisor.nativeMigration ||
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
                    {translate("devices.update")}</button>
                )}
            </div>
          </div>
          {uptime && (
            <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
              {translate("devices.uptime")} {uptime}
            </p>
          )}
          {supervisor?.installedVersion &&
            supervisor.installedVersion !== supervisor.runningVersion && (
              <p className="mt-2 text-xs">
                {translate("devices.installed_7bb440")} {supervisor.installedVersion}{translate("devices.running")}{' '}
                {supervisor.runningVersion}{translate("devices.checkUpdatesToBringTheInstallationAnd")}</p>
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
                ? translate("devices.waitingForTheDeviceToReconnect")
                : supervisorJobText(supervisor.job)}
              {reconnectSince && clock - reconnectSince > 30_000 && (
                <span className="mt-1 block text-[var(--status-warning-fg)]">
                  {translate("devices.theDeviceHasNotReturnedYetInstallation")}</span>
              )}
              {active(supervisor.job) && (
                <span className="mt-1 block text-[var(--theme-fg-muted)]">
                  {translate("devices.controlsAreTemporarilyDisabledUntilThisOperation")}</span>
              )}
            </p>
          )}
        </>
      )}
      {view !== 'device' && (
        <>
          <div
            className="settings-harness-picker flex flex-wrap gap-2 pb-4"
            role="group"
            aria-label={translate("devices.chooseHarness")}
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
            ].map((h) => {
              const status = 'base' in h ? harnessStatus(h) : null;
              return (
                <button
                  key={h.id}
                  type="button"
                  aria-pressed={selectedHarness === h.id}
                  title={status ? statusText(status) : undefined}
                  className={`inline-flex min-h-10 items-center gap-1.5 rounded-xl border px-3 text-xs transition ${selectedHarness === h.id ? 'border-[var(--theme-accent-border)] bg-[var(--theme-accent-soft)] text-[var(--theme-accent-strong)]' : 'border-[var(--theme-border)] hover:bg-[var(--theme-hover)]'}`}
                  onClick={() => setSelectedHarness(h.id)}
                >
                  {status && (
                    <span aria-hidden className={`h-1.5 w-1.5 shrink-0 rounded-full ${statusDot[status]}`} />
                  )}
                  {h.name}
                </button>
              );
            })}
          </div>
          {harnesses.filter((h) => h.id === selectedHarness).map(row)}
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
          title={`${confirm.action === 'restart' ? translate("devices.restart_b134bd") : confirm.installing ? translate("devices.install") : translate("devices.update")} ${confirm.name}`}
          busy={busy}
          onClose={() => setConfirm(null)}
          description={
            confirm.action === 'restart'
              ? translate("devices.theDeviceWillBrieflyDisconnectRunningTasks")
              : confirm.id
                ? confirm.installing
                  ? translate("devices.installThisComponentOnTheSelectedDevice")
                  : translate("devices.updateTheSelectedInstallationThenReloadIts")
                : translate("devices.theSupervisorWillBrieflyDisconnectAnIndependent")
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
              ? translate("devices.starting")
              : confirm.action === 'restart'
                ? translate("devices.restart_b134bd")
                : confirm.installing
                  ? translate("devices.install")
                  : translate("devices.update")}
          </button>
        </FormDialog>
      )}
    </section>
  );
}
