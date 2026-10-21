import { useHostScope } from '../../lib/hostCapabilities';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import type { FormEvent, ReactNode } from 'react';
import { useEffect, useId, useRef, useState } from 'react';
import { Link } from 'react-router-dom';

import {
  AgentBackendDto,
  AgentBackendIdDto,
  defaultAgentBackendId,
  ModelOptionDto,
  ReasoningEffortDto,
  ThreadDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import { FormDialog } from '../../components/FormDialog';
import { useAppShellNav } from '../../components/AppShellNavContext';
import { dshRunModeHint, dshRunModeName, type DshRunMode } from '../../components/DshHarnessPanel';
import {
  ApiError,
  createThread,
  createWorkspaceThread,
  fetchAgentBackendAgents,
  fetchAgentBackends,
  fetchAgentBackendModels,
  fetchAgentBackendModelsFor,
  fetchAgentHarnessCatalog,
  fetchRelayAccess,
  fetchWorkspace,
  fetchWorkspaces,
  installOrUpdateAgentBackend,
  postThreadHarnessAction,
  relayModeActive,
} from '../../lib/api';
import { workspaceShare } from '../../lib/shareRules';
import { currentRelayDeviceIdFromPath, currentRelayScopedPath } from '../../lib/relayRoutes';

function canStart(backend: AgentBackendDto) {
  return backend.enabled && backend.capabilities.sessions.resume && backend.capabilities.turns.start;
}

function chooseProvider(backends: AgentBackendDto[], preferred: AgentBackendIdDto) {
  const match = backends.find((backend) => backend.provider === preferred);
  if (match && canStart(match)) {
    return match.provider;
  }
  return backends.find(canStart)?.provider ?? defaultAgentBackendId;
}

function pickModel(models: ModelOptionDto[]) {
  return models.find((entry) => entry.isDefault) ?? models[0] ?? null;
}

function pickReadyAgent(agents: ModelOptionDto[], preferred?: string) {
  const ready = (entry: ModelOptionDto) => (entry.acpAgent?.availability === 'ready' || entry.acpAgent?.availability === 'adapter_missing');
  return (
    agents.find((entry) => entry.model === preferred && ready(entry)) ??
    agents.find((entry) => entry.isDefault && ready(entry)) ??
    agents.find(ready) ??
    null
  );
}

function errorText(caught: unknown, fallback: string) {
  if (caught instanceof ApiError) {
    return caught.payload.message;
  }
  return caught instanceof Error ? caught.message : fallback;
}

/** The thread exists, but DSH did not accept its selected run mode. */
class RunModeNotApplied extends Error {
  constructor(readonly cause: unknown) {
    super(errorText(cause, ''));
  }
}

function Field({
  id,
  label,
  children,
}: {
  id?: string;
  label: string;
  children: ReactNode;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div>
      <label className="host-form-label text-xs font-medium" htmlFor={id}>
        {label}
      </label>
      {children}
    </div>
  );
}

export function ThreadCreateForm({
  initialWorkspaceId,
  initialTitle = '',
  onCreated,
  onCancel,
  variant = 'panel',
}: {
  initialWorkspaceId?: string | null | undefined;
  initialTitle?: string | null | undefined;
  onCreated: (thread: ThreadDto) => void;
  onCancel?: () => void;
  variant?: 'panel' | 'dialog';
}) {
  const { locale: i18nLocale } = useI18n();
  const shellNav = useAppShellNav();
  const formId = useId();
  const compact = variant === 'dialog';
  const controlClass = compact
    ? 'host-form-control mt-1.5 h-10 w-full rounded-lg border px-3 text-sm outline-none transition'
    : 'host-form-control mt-2 w-full rounded-lg border px-4 py-3 outline-none transition';
  const defaultBackend = shellNav?.defaultBackend ?? defaultAgentBackendId;

  const hostScope = useHostScope();
  const [workspaces, setWorkspaces] = useState<WorkspaceDto[]>([]);
  const [backends, setBackends] = useState<AgentBackendDto[]>([]);
  const [agentOptions, setAgentOptions] = useState<ModelOptionDto[]>([]);
  const [models, setModels] = useState<ModelOptionDto[]>([]);
  const [provider, setProvider] = useState<AgentBackendIdDto>(defaultBackend);
  const [workspaceId, setWorkspaceId] = useState('');
  // A workspace share starts threads only in its workspace, through that workspace's route.
  const [sharedWorkspaceId, setSharedWorkspaceId] = useState<string | null>(null);
  const [agentId, setAgentId] = useState('');
  const [model, setModel] = useState('');
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffortDto | null>(null);
  // DSH run modes (agent presets); applied to the new thread before its first turn.
  const [runModes, setRunModes] = useState<DshRunMode[]>([]);
  const [runMode, setRunMode] = useState('');
  // Created, but its run mode still has to be applied: retry instead of creating again.
  const [created, setCreated] = useState<ThreadDto | null>(null);
  useEffect(() => setCreated(null), [provider, agentId, workspaceId]);
  const [title, setTitle] = useState(() => initialTitle ?? '');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [runtimeBusyProvider, setRuntimeBusyProvider] = useState<AgentBackendIdDto | null>(null);
  const [installingAgentId, setInstallingAgentId] = useState<string | null>(null);
  // Why loading or an action failed, as a mark after its control: the runtime
  // button for 'runtime', Create for the rest ('load' and 'models' can retry).
  const [error, setError] = useState<{ at: 'load' | 'models' | 'runtime' | 'create'; message: string } | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [modelsAttempt, setModelsAttempt] = useState(0);
  const [adapterPrompt, setAdapterPrompt] = useState<{ entry: ModelOptionDto; createAfter: boolean } | null>(null);
  const [adapterError, setAdapterError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const selectedBackend = backends.find((backend) => backend.provider === provider);
  const acpAdvertised = backends.some((backend) => backend.provider === 'acp');
  const selectedWorkspace = workspaces.find((workspace) => workspace.id === workspaceId) ?? null;
  const selectedModel = models.find((entry) => entry.model === model) ?? null;
  const isAcpAgentSelection =
    provider === 'acp' && agentOptions.some((entry) => entry.selectionKind === 'agent');
  const selectedAgent = agentOptions.find((entry) => entry.model === agentId) ?? null;

  function applyModels(records: ModelOptionDto[]) {
    const next = pickModel(records);
    setModels(records);
    setModel(next?.model ?? '');
    setReasoningEffort(next?.defaultReasoningEffort ?? null);
  }

  useEffect(() => {
    setTitle(initialTitle ?? '');
  }, [initialTitle]);

  useEffect(() => {
    let cancelled = false;
    const device = currentRelayDeviceIdFromPath();
    const shared = device && relayModeActive() && initialWorkspaceId && !hostScope
      ? fetchRelayAccess({ deviceId: device, workspaceId: initialWorkspaceId })
        .then((access) => (workspaceShare(access) ? access.workspaceId : null), () => null)
      : Promise.resolve(null);
    shared
      // A workspace share's first device request names its workspace, so the
      // encrypted handshake runs under that resource before the harness list.
      .then(async (sharedId): Promise<[WorkspaceDto[], AgentBackendDto[], string | null]> => {
        if (!sharedId) return [...await Promise.all([fetchWorkspaces(), fetchAgentBackends()]), null];
        const record = await fetchWorkspace(sharedId);
        return [[record], await fetchAgentBackends(), sharedId];
      })
      .then(([workspaceRecords, backendRecords, sharedId]) => {
        if (cancelled) {
          return;
        }
        setSharedWorkspaceId(sharedId);
        setProvider(chooseProvider(backendRecords, defaultBackend));
        setBackends(backendRecords);
        setWorkspaces(workspaceRecords);
        const requested = initialWorkspaceId ?? null;
        setWorkspaceId(
          requested === null
            ? (workspaceRecords[0]?.id ?? '')
            : workspaceRecords.some((workspace) => workspace.id === requested)
              ? requested
              : '',
        );
      })
      .catch((caught) => {
        if (!cancelled) {
          setError({ at: 'load', message: errorText(caught, translate("workbench.unableToLoadCreationFormData")) });
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [defaultBackend, initialWorkspaceId, hostScope, loadAttempt]);

  useEffect(() => {
    if (!provider || !workspaceId) {
      return;
    }
    let cancelled = false;
    setModels([]);
    setModel('');
    setReasoningEffort(null);
    setError(null);
    if (provider === 'acp') {
      fetchAgentBackendAgents(provider)
        .then((agents) => {
          if (cancelled) {
            return;
          }
          setAgentOptions(agents);
          setAgentId((current) => pickReadyAgent(agents, current)?.model ?? '');
        })
        .catch((caught) => {
          if (!cancelled) {
            setAgentOptions([]);
            setAgentId('');
            setError({ at: 'models', message: errorText(caught, translate("workbench.unableToLoadACPAgents")) });
          }
        });
      return () => {
        cancelled = true;
      };
    }
    setAgentOptions([]);
    setAgentId('');
    fetchAgentBackendModelsFor(
      provider,
      selectedWorkspace ? { cwd: selectedWorkspace.absPath } : {},
    )
      .then((records) => {
        if (!cancelled) {
          applyModels(records);
        }
      })
      .catch((caught) => {
        if (!cancelled) {
          applyModels([]);
          setError({ at: 'models', message: errorText(caught, translate("workbench.unableToLoadBackendModels")) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [provider, workspaceId, selectedWorkspace?.absPath, modelsAttempt]);

  useEffect(() => {
    if (provider !== 'acp' || !agentId || !selectedWorkspace) {
      return;
    }
    let cancelled = false;
    setModels([]);
    setModel('');
    setReasoningEffort(null);
    setRunModes([]);
    setRunMode('');
    setError(null);
    if (selectedAgent?.acpAgent?.availability === 'adapter_missing') return;
    fetchAgentBackendModelsFor('acp', { agentId, cwd: selectedWorkspace.absPath })
      .then((records) => {
        if (cancelled) return;
        applyModels(records);
        if (hostScope || agentId !== 'deepseek') return;
        // The model probe booted DSH, so its run modes are cached by now.
        // They are optional: without them the thread uses DSH's default mode.
        // Applying one is a device-owner action on Relay.
        const device = currentRelayDeviceIdFromPath();
        const owner = device
          ? fetchRelayAccess({ deviceId: device }).then((access) => access.kind === 'owner', () => false)
          : Promise.resolve(true);
        return Promise.all([owner, fetchAgentHarnessCatalog<{ runModes?: DshRunMode[] }>('acp', {
          agentId,
          cwd: selectedWorkspace.absPath,
        })]).then(([isOwner, catalog]) => {
          if (cancelled || !isOwner) return;
          const modes = catalog?.runModes ?? [];
          setRunModes(modes);
          setRunMode(modes.find((mode) => mode.isDefault && !mode.broken)?.id ?? '');
        }, () => undefined);
      })
      .catch((caught) => {
        if (!cancelled) {
          setError({ at: 'models', message: errorText(caught, translate("workbench.unableToLoadAgentModels")) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, provider, selectedWorkspace, selectedAgent?.acpAgent?.availability, modelsAttempt]);

  async function reloadBackends(nextProvider: AgentBackendIdDto = provider) {
    const records = await fetchAgentBackends();
    setBackends(records);
    const requested = records.find((backend) => backend.provider === nextProvider);
    const selected =
      requested && canStart(requested)
        ? nextProvider
        : chooseProvider(records, defaultBackend);
    setProvider(selected);
    if (selected === 'acp') {
      const agents = await fetchAgentBackendAgents('acp');
      setAgentOptions(agents);
      setAgentId(pickReadyAgent(agents)?.model ?? '');
      return;
    }
    applyModels(await fetchAgentBackendModels(selected));
  }

  async function handleRuntimeAction(backend: AgentBackendDto) {
    const action = backend.installation.installed ? 'update' : 'install';
    setRuntimeBusyProvider(backend.provider);
    setError(null);
    try {
      await installOrUpdateAgentBackend(backend.provider, action);
      await reloadBackends(backend.provider);
    } catch (caught) {
      setError({ at: 'runtime', message: errorText(caught, `Unable to ${action} ${backend.displayName}.`) });
      try {
        await reloadBackends(provider);
      } catch {
        /* keep install error */
      }
    } finally {
      setRuntimeBusyProvider(null);
    }
  }

  async function createSelectedThread(nextModel = model, nextEffort = reasoningEffort) {
    const trimmed = title.trim();
    // A retry after a failed run-mode selection reuses the thread already created.
    const input = {
      workspaceId, provider, ...(provider === 'acp' ? { agentId } : {}), model: nextModel,
      ...(nextEffort ? { reasoningEffort: nextEffort } : {}), approvalMode: 'yolo',
      ...(trimmed ? { title: trimmed } : {}),
    } as const;
    const thread = created ?? await (sharedWorkspaceId && sharedWorkspaceId === workspaceId
      ? createWorkspaceThread(sharedWorkspaceId, input)
      : createThread(input));
    const mode = runModes.find((entry) => entry.id === runMode);
    // Always explicit: the cached default can be stale once DSH's own default changes.
    if (provider === 'acp' && agentId === 'deepseek' && mode) {
      setCreated(thread);
      try {
        await postThreadHarnessAction(thread.id, { kind: 'selectRunMode', id: mode.id });
      } catch (caught) {
        throw new RunModeNotApplied(caught);
      }
    }
    if (!alive.current) return;
    setCreated(null);
    onCreated(thread);
  }
  function askAdapterInstall(entry: ModelOptionDto, createAfter: boolean) {
    setAdapterError(null);
    setAdapterPrompt({ entry, createAfter });
  }
  async function confirmAdapterInstall() {
    if (!adapterPrompt || busy) return;
    const { entry, createAfter } = adapterPrompt;
    setBusy(true);
    setInstallingAgentId(entry.id);
    setAdapterError(null);
    try {
      await installOrUpdateAgentBackend('acp', 'install', entry.id);
      if (!alive.current) return;
      const agents = await fetchAgentBackendAgents('acp');
      if (!alive.current) return;
      const installed = agents.find(candidate => candidate.id === entry.id);
      if (installed?.acpAgent?.availability !== 'ready') throw new Error(translate('workbench.adapterInstallNotReady'));
      const records = await fetchAgentBackendModelsFor('acp', { agentId: installed.model, cwd: selectedWorkspace?.absPath ?? null });
      if (!alive.current) return;
      const next = records.find(record => record.model === model) ?? pickModel(records);
      if (!next) throw new Error(translate('workbench.unableToLoadAgentModels'));
      setAgentOptions(agents);
      setAgentId(installed.model);
      applyModels(records);
      if (createAfter) await createSelectedThread(next.model, next.defaultReasoningEffort ?? null);
      if (alive.current) setAdapterPrompt(null);
    } catch (caught) {
      if (alive.current) setAdapterError(errorText(caught, translate('workbench.adapterInstallFailed')));
    } finally {
      if (alive.current) { setInstallingAgentId(null); setBusy(false); }
    }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedBackend || !canStart(selectedBackend)) {
      setError({ at: 'create', message: translate("workbench.chooseAnAvailableBackendBeforeCreatingA") });
      return;
    }
    if (hostScope && selectedAgent?.acpAgent?.availability === 'adapter_missing') return;
    if (provider === 'acp' && selectedAgent?.acpAgent?.availability === 'adapter_missing') {
      askAdapterInstall(selectedAgent, true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await createSelectedThread();
    } catch (caught) {
      setError({
        at: 'create',
        message: caught instanceof RunModeNotApplied
          ? translate("workbench.dshRunModeNotApplied", { value1: errorText(caught.cause, '') })
          : errorText(caught, translate("workbench.unableToCreateThread")),
      });
    } finally {
      setBusy(false);
    }
  }

  if (loading) {
    return (
      <div className="host-empty-state rounded-lg border px-6 py-12 text-center">
        {translate("workbench.loadingCreationForm")}</div>
    );
  }

  const backendUnavailable = selectedBackend && !canStart(selectedBackend);
  const selectedBackendAction = selectedBackend?.installation.installed ? translate("workbench.update") : translate("workbench.install");
  const selectedBackendActionAvailable = !hostScope && selectedBackend
    ? selectedBackend.installation.installed
      ? Boolean(selectedBackend.installation.updateCommand)
      : Boolean(selectedBackend.installation.installCommand)
    : false;
  const selectedBackendBusy = selectedBackend
    ? runtimeBusyProvider === selectedBackend.provider || selectedBackend.installation.busy
    : false;
  const selectedBackendBusyLabel = selectedBackend?.installation.installed
    ? translate("workbench.updating")
    : translate("workbench.installing");

  return (
    <form
      onSubmit={handleSubmit}
      className={compact ? 'thread-create-form min-h-0 space-y-3 overflow-y-auto overscroll-contain pr-1 text-sm' : 'space-y-5'}
    >
      {compact ? (
        <div className="pr-8">
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">{translate("workbench.createNewChat")}</h2>
          <p className="mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("workbench.chooseTheWorkspaceAndAgentForThis")}</p>
        </div>
      ) : null}

      <fieldset>
        <legend className="host-form-label text-xs font-medium">{translate("workbench.backend")}</legend>
        {compact ? (
          <select
            id={`${formId}-thread-backend`}
            disabled={busy || runtimeBusyProvider !== null}
            value={provider}
            onChange={(event) => setProvider(event.target.value as AgentBackendIdDto)}
            className={controlClass}
          >
            {backends.map((backend) => (
              <option key={backend.provider} value={backend.provider} disabled={!canStart(backend)}>
                {backend.displayName}
                {canStart(backend) ? '' : translate("workbench.notAvailable")}
              </option>
            ))}
            {!acpAdvertised ? (
              <option value="acp" disabled>
                {translate("workbench.aCPAgentEnableOnDevice")}</option>
            ) : null}
          </select>
        ) : (
          <div aria-label={translate("workbench.backend")} className="product-segmented mt-2 !grid w-full grid-cols-2 sm:!flex" role="radiogroup">
            {backends.map((backend) => {
              const selected = backend.provider === provider;
              return (
                <label
                  className="product-segment relative min-w-0 flex-1 disabled:cursor-not-allowed disabled:opacity-45"
                  key={backend.provider}
                >
                  <input
                    checked={selected}
                    className="sr-only"
                    disabled={!canStart(backend) || busy || runtimeBusyProvider !== null}
                    name={`${formId}-backend`}
                    onChange={() => setProvider(backend.provider)}
                    type="radio"
                    value={backend.provider}
                  />
                  {backend.displayName}
                </label>
              );
            })}
          </div>
        )}

        {selectedBackend && provider !== 'acp' ? (
          <div className="mt-3 flex flex-col gap-2 border-b border-[var(--theme-border)] pb-4 sm:flex-row sm:items-center sm:justify-between">
            <p className="flex min-w-0 items-center gap-1.5 text-xs leading-5 text-[var(--theme-fg-muted)]">
              {selectedBackend.installation.installed
                ? translate("workbench.installed", { value1: selectedBackend.installation.installedVersion ? `, version ${selectedBackend.installation.installedVersion}` : '' })
                : translate("workbench.runtimeIsNotInstalled")}
              {/* The runtime's last failure is behind a mark, not a sentence. */}
              {!selectedBackend.installation.installed && (selectedBackend.installation.lastError ?? selectedBackend.status.lastError)
                ? <ErrorMark message={(selectedBackend.installation.lastError ?? selectedBackend.status.lastError)!} />
                : null}
            </p>
            {selectedBackendActionAvailable ? (
              <span className="flex shrink-0 items-center gap-2">
                <button
                  aria-label={`${selectedBackendAction} ${selectedBackend.displayName}`}
                  className="host-secondary-button min-h-10 shrink-0 rounded-md border px-3 text-xs font-semibold transition disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={busy || selectedBackendBusy || runtimeBusyProvider !== null}
                  onClick={() => void handleRuntimeAction(selectedBackend)}
                  type="button"
                >
                  {selectedBackendBusy ? selectedBackendBusyLabel : translate("workbench.runtime", { value1: selectedBackendAction })}
                </button>
                {error?.at === 'runtime' ? <ErrorMark message={error.message} /> : null}
              </span>
            ) : null}
          </div>
        ) : null}
        {!acpAdvertised ? (
          <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("workbench.aCPIsNotEnabledOnThisSupervisor")}<code className="mx-1 font-mono">acp</code>
            {translate("workbench.to")} <code className="font-mono">POCKYMOE_ENABLED_AGENT_PROVIDERS</code>.
          </p>
        ) : null}
      </fieldset>

      <Field id={`${formId}-thread-workspace`} label={translate("workbench.workspace")}>
        <select
          id={`${formId}-thread-workspace`}
          disabled={busy}
          value={workspaceId}
          onChange={(event) => setWorkspaceId(event.target.value)}
          className={controlClass}
        >
          {!workspaceId ? <option value="">{translate("workbench.chooseAWorkspace")}</option> : null}
          {workspaces.map((workspace) => (
            <option key={workspace.id} value={workspace.id}>
              {workspace.label} · {workspace.absPath}
            </option>
          ))}
        </select>
        {!hostScope && workspaces.length === 0 ? (
          <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("workbench.addAWorkspaceBeforeCreatingAThread")}{' '}
            <Link className="font-semibold text-[var(--theme-accent-strong)] hover:underline" to={currentRelayScopedPath('/workspaces/new')}>
              {translate("workbench.addWorkspace")}</Link>
          </p>
        ) : initialWorkspaceId && !workspaces.some((workspace) => workspace.id === initialWorkspaceId) ? (
          <p className="mt-2 text-xs leading-5 text-[var(--status-warning-fg)]" role="status">
            {translate("workbench.theRequestedWorkspaceIsUnavailableChooseAnother")}</p>
        ) : null}
      </Field>

      {isAcpAgentSelection ? (
        <fieldset>
          <legend className="host-form-label text-xs font-medium">{translate("workbench.agent")}</legend>
          <div
            className={`mt-2 ${compact ? 'max-h-32' : 'max-h-64'} divide-y divide-[var(--theme-border)] overflow-y-auto overscroll-contain rounded-lg border border-[var(--theme-border)] bg-[var(--theme-surface)]`}
            role="radiogroup"
            aria-label={translate("workbench.agent")}
          >
            {agentOptions.map((entry) => {
              const meta = entry.acpAgent;
              const ready = meta?.availability === 'ready' || meta?.availability === 'adapter_missing';
              const adapterMissing = meta?.availability === 'adapter_missing';
              const installing = installingAgentId === entry.id || meta?.busy === true;
              const selected = entry.model === agentId;
              const statusLabel =
                meta?.availability === 'base_missing'
                  ? translate("workbench.baseAgentMissing")
                  : adapterMissing
                    ? translate("workbench.adapterNeeded")
                    : meta?.availability === 'server_unavailable'
                      ? translate("workbench.aCPUnavailable")
                      : meta?.statusMessage === 'Ready' ? translate("workbench.ready") : meta?.statusMessage?.startsWith('ACP verification failed:') ? translate("workbench.connectionFailed") : translate("workbench.installed_7bb440");
              return (
                <div key={entry.id} className="flex min-w-0 items-stretch gap-2 px-2 py-2">
                  <label
                    title={meta?.statusMessage}
                    className={`product-radio-option relative min-w-0 flex-1 rounded-md px-2.5 py-2 text-left outline-none transition ${
                      selected
                        ? 'bg-[var(--theme-surface-strong)] text-[var(--theme-fg)]'
                        : ready
                          ? 'text-[var(--theme-fg)] hover:bg-[var(--theme-hover)]'
                          : 'cursor-not-allowed text-[var(--theme-fg-muted)] opacity-70'
                    }`}
                  >
                    <input
                      checked={selected}
                      className="sr-only"
                      disabled={!ready || busy || installingAgentId !== null}
                      name={`${formId}-agent`}
                      onChange={() => setAgentId(entry.model)}
                      type="radio"
                      value={entry.model}
                    />
                    <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="truncate font-medium">{entry.displayName}</span>
                      <span className="host-pill rounded-full px-2 py-0.5 text-[0.68rem]">{statusLabel}</span>
                    </span>
                    <span className="mt-1 block truncate font-mono text-[0.68rem] text-[var(--theme-fg-muted)]">
                      {meta?.serverCommand ?? translate("workbench.unavailable")}
                    </span>
                  </label>
                  {!hostScope && adapterMissing && meta?.installCommand ? (
                    <button
                      type="button"
                      onClick={() => askAdapterInstall(entry, false)}
                      disabled={busy || installing || installingAgentId !== null}
                      className="host-secondary-button my-auto min-h-11 shrink-0 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      {installing ? translate("workbench.installing") : translate("workbench.installAdapter")}
                    </button>
                  ) : null}
                </div>
              );
            })}
          </div>
        </fieldset>
      ) : null}

      <div className={selectedModel?.supportedReasoningEfforts.length ? 'grid gap-5 sm:grid-cols-2' : ''}>
        <Field id={`${formId}-thread-model`} label={translate("workbench.model")}>
          <select
            id={`${formId}-thread-model`}
            value={model}
            disabled={busy || models.length === 0}
            onChange={(event) => {
              const next = models.find((entry) => entry.model === event.target.value) ?? null;
              setModel(event.target.value);
              setReasoningEffort((current) =>
                current && next?.supportedReasoningEfforts.some((entry) => entry.reasoningEffort === current)
                  ? current
                  : (next?.defaultReasoningEffort ?? null),
              );
            }}
            className={controlClass}
          >
            {models.length === 0 ? <option value="">{translate(selectedAgent?.acpAgent?.availability === 'adapter_missing' ? "workbench.adapterNeeded" : "workbench.noModelsAvailable")}</option> : null}
            {models.map((entry) => (
              <option key={entry.id} value={entry.model}>
                {entry.displayName}
              </option>
            ))}
          </select>
        </Field>

        {selectedModel && selectedModel.supportedReasoningEfforts.length > 0 ? (
          <Field id={`${formId}-thread-effort`} label={translate("workbench.reasoningEffort")}>
            <select
              id={`${formId}-thread-effort`}
              disabled={busy}
              value={reasoningEffort ?? ''}
              onChange={(event) =>
                setReasoningEffort((event.target.value || null) as ReasoningEffortDto | null)
              }
              className={controlClass}
            >
              {selectedModel.supportedReasoningEfforts.map((entry) => (
                <option key={entry.reasoningEffort} value={entry.reasoningEffort}>
                  {entry.reasoningEffort || translate("workbench.providerDefault")}
                </option>
              ))}
            </select>
          </Field>
        ) : null}

        {provider === 'acp' && agentId === 'deepseek' && runModes.length > 0 ? (
          <Field id={`${formId}-thread-run-mode`} label={translate("settings.dshRunMode")}>
            <select
              id={`${formId}-thread-run-mode`}
              disabled={busy || Boolean(created)}
              value={runMode}
              onChange={(event) => setRunMode(event.target.value)}
              className={controlClass}
            >
              {runModes.map((mode) => (
                <option key={mode.id} value={mode.id} disabled={Boolean(mode.broken)}>{dshRunModeName(mode)}</option>
              ))}
            </select>
            <p className="host-muted mt-1 text-xs">
              {dshRunModeHint(runModes.find((mode) => mode.id === runMode))}
            </p>
          </Field>
        ) : null}
      </div>

      <Field id={`${formId}-thread-title`} label={translate("workbench.title")}>
        <input
          id={`${formId}-thread-title`}
          disabled={busy}
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          placeholder={translate("workbench.optionalFallsBackToFirstPrompt")}
          className={controlClass}
        />
      </Field>

      {adapterPrompt ? (
        <FormDialog title={translate('workbench.adapterInstallTitle')}
          description={translate('workbench.adapterInstallDescription', { agent: adapterPrompt.entry.displayName })}
          busy={busy} onClose={() => { setAdapterPrompt(null); setAdapterError(null); }}>
          <div className="mt-4 flex items-center justify-end gap-2">
            <button type="button" disabled={busy} onClick={() => setAdapterPrompt(null)} className="host-secondary-button min-h-11 rounded-md border px-4 text-sm disabled:opacity-50">{translate('workbench.cancel')}</button>
            <button type="button" disabled={busy} onClick={() => void confirmAdapterInstall()} className="ui-action-primary min-h-11 rounded-md px-4 text-sm font-semibold disabled:opacity-50">
              {busy ? translate('workbench.installing') : adapterError ? translate('workbench.adapterInstallRetry') : translate('workbench.adapterInstallConfirm')}
            </button>
            {adapterError ? <ErrorMark message={adapterError} /> : null}
          </div>
        </FormDialog>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 pt-1">
        <button
          type="submit"
          disabled={
            busy ||
            Boolean(hostScope && selectedAgent?.acpAgent?.availability === 'adapter_missing') ||
            !selectedBackend ||
            !canStart(selectedBackend) ||
            !workspaceId ||
            (selectedAgent?.acpAgent?.availability !== 'adapter_missing' && (!model || !selectedModel)) ||
            (provider === 'acp' && !['ready', 'adapter_missing'].includes(selectedAgent?.acpAgent?.availability ?? ''))
          }
          className={
            compact
              ? 'ui-action-primary min-h-11 rounded-md px-4 text-sm font-semibold transition disabled:cursor-not-allowed'
              : 'ui-action-primary min-h-11 rounded-md px-5 font-semibold transition disabled:cursor-not-allowed'
          }
        >
          {busy ? translate("workbench.creating")
            : created ? translate("workbench.retryRunMode")
            : translate("workbench.createThread_ea3fa3")}
        </button>
        {error && error.at !== 'runtime' ? (
          <ErrorMark
            message={error.message}
            onRetry={error.at === 'load' ? () => { setError(null); setLoading(true); setLoadAttempt((value) => value + 1); }
              : error.at === 'models' ? () => { setError(null); setModelsAttempt((value) => value + 1); }
              : undefined}
          />
        ) : backendUnavailable ? (
          <ErrorMark kind="unavailable" message={translate("workbench.selectAnAvailableBackendBeforeCreatingA")} />
        ) : null}
        {created && !busy ? (
          <button
            type="button"
            onClick={() => onCreated(created)}
            className="host-secondary-button min-h-11 rounded-md border px-3 text-sm font-medium transition"
          >
            {translate("workbench.openCreatedThread")}
          </button>
        ) : null}
        {onCancel ? (
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className={
              compact
                ? 'host-secondary-button min-h-11 rounded-md border px-3 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-60'
                : 'host-secondary-button min-h-11 rounded-md border px-5 font-medium transition disabled:cursor-not-allowed disabled:opacity-60'
            }
          >
            {translate("workbench.cancel")}</button>
        ) : null}
      </div>
    </form>
  );
}
