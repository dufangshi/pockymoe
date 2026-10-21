import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { useEffect, useState } from 'react';

// Adapter data is metadata only: names and descriptions render as text.
export interface DshPlugin {
  id: string;
  name: string;
  module: string;
  description: string;
  enabled: boolean;
  active: boolean;
  readOnly: string | null;
}
export interface DshBundle {
  name: string;
  version: string | null;
  description: string;
  enabled: boolean;
  removable: boolean;
  readOnly: string | null;
}
/** A DSH agent preset ("run mode"): the tool set and persona of a session. */
export interface DshRunMode {
  id: string;
  name: string | null;
  description: string | null;
  isDefault: boolean;
  broken: unknown;
}
export interface DshCommand {
  name: string;
  description: string;
  hint: string | null;
}
export interface DshHarnessInfo {
  kind: 'dsh';
  version: string | null;
  profile: string;
  /** `native` when DSH booted with its own Web bundle (run modes, console). */
  composition?: 'native' | 'acp' | null;
  compositionError?: string | null;
  runModes?: DshRunMode[];
  features?: Partial<Record<'runModes' | 'console' | 'commands', boolean>>;
  permissionPresets: string[];
  plugins: DshPlugin[];
  bundles: DshBundle[];
  providers: { id: string; name: string; declared: boolean }[];
  session?: {
    running: boolean;
    /** DSH fixes the run mode once the session's first turn starts. */
    presetLocked?: boolean;
    commands?: DshCommand[];
    projections: {
      agentPreset?: string | null;
      plan?: { active: boolean; pending: boolean } | null;
      permissions?: { currentValue: string } | null;
      goal?: {
        goal: { objective: string; phase: string };
        roundsStarted: number;
      } | null;
      todos?: { content: string; status: string }[] | null;
    };
  };
}
type SettingValue = string | number | boolean | null;
export interface DshSettingsView {
  ns: string;
  revision: number;
  fields: {
    key: string;
    type: 'string' | 'number' | 'boolean';
    value: SettingValue;
    overridden: boolean;
  }[];
}
export type DshPanelAction =
  | { kind: 'refresh' }
  | { kind: 'settings' }
  | { kind: 'stop' }
  | { kind: 'restart' }
  | { kind: 'selectRunMode'; id: string }
  | { kind: 'console' }
  | { kind: 'command'; line: string }
  | { kind: 'setPluginEnabled'; id: string; enabled: boolean }
  | { kind: 'setBundleEnabled'; name: string; enabled: boolean }
  | {
      kind: 'updateSetting';
      ns: string;
      key: string;
      value: SettingValue;
      revision: number;
    };
/** Loopback proxy port and token path of the session's native DSH Web UI,
 * plus the device's preview mapping when it is connected to a Relay. */
export interface DshConsoleTarget {
  port: number;
  path: string;
  mappingId?: string;
  leaseId?: string;
  expiresAt?: number;
}
export interface DshPanelResult {
  result?: {
    application?: string;
    /** DSH command outcome. */
    result?: { kind?: string; text?: string };
  } | null;
  harness?: DshHarnessInfo;
  settings?: DshSettingsView[];
  console?: DshConsoleTarget;
}

export function isDshHarness(info: unknown): info is DshHarnessInfo {
  return (info as { kind?: unknown } | null)?.kind === 'dsh';
}

// DSH's built-in run modes; custom modes (made in Creator mode) show their own name.
const RUN_MODES = {
  standard: ['settings.dshRunModeStandard', 'settings.dshRunModeStandardHint'],
  ptc: ['settings.dshRunModePtc', 'settings.dshRunModePtcHint'],
  minimal: ['settings.dshRunModeMinimal', 'settings.dshRunModeMinimalHint'],
  cordis: ['settings.dshRunModeCordis', 'settings.dshRunModeCordisHint'],
} as const;
const builtinRunMode = (id: string) =>
  Object.hasOwn(RUN_MODES, id) ? RUN_MODES[id as keyof typeof RUN_MODES] : null;

export function dshRunModeName(mode: DshRunMode) {
  const known = builtinRunMode(mode.id);
  const name = known ? translate(known[0]) : mode.name ?? mode.id;
  if (mode.broken) {
    // DSH or the bridge says why this mode cannot run on the device.
    const reason = typeof mode.broken === 'string' ? mode.broken : translate('settings.dshRunModeBrokenUnknown');
    return translate('settings.dshRunModeBroken', { value1: name, value2: reason });
  }
  return mode.isDefault ? translate('settings.dshRunModeDefault', { value1: name }) : name;
}

export function dshRunModeHint(mode: DshRunMode | undefined) {
  if (!mode) return '';
  const known = builtinRunMode(mode.id);
  return known ? translate(known[1]) : mode.description ?? '';
}

// Mirrors the runtime: thread controls own plan mode, sandbox, goals and compaction.
const THREAD_COMMANDS = new Set(['plan', 'permission', 'goal', 'compact']);
// Web-only commands whose result is a browser action (the log download).
const CONSOLE_COMMANDS = new Set(['export']);
// With DSH's default telemetry mode, feedback uploads the session history.
const UPLOAD_COMMANDS = new Set(['feedback']);

function DshCommandRow({
  command,
  disabled,
  error,
  onRun,
}: {
  command: DshCommand;
  disabled: boolean;
  /** Why this command's last run failed. */
  error: string | null;
  onRun: (line: string) => void;
}) {
  const [args, setArgs] = useState('');
  const owner = THREAD_COMMANDS.has(command.name) ? 'settings.dshThreadControl'
    : CONSOLE_COMMANDS.has(command.name) ? 'settings.dshInConsole' : null;
  return (
    <li className="space-y-1 text-xs">
      <form
        className="flex items-center gap-2"
        onSubmit={async (event) => {
          event.preventDefault();
          const line = `/${command.name}${args.trim() ? ` ${args.trim()}` : ''}`;
          if (UPLOAD_COMMANDS.has(command.name) && !(await confirmAsync({
            title: `/${command.name}`,
            description: translate('settings.dshFeedbackConfirm'),
            confirmLabel: translate('settings.dshFeedbackSend'),
          }))) return;
          onRun(line);
        }}
      >
        <span className="font-mono">/{command.name}</span>
        {owner ? (
          <span className="ml-auto text-[var(--theme-fg-muted)]">{translate(owner)}</span>
        ) : (
          <>
            {command.hint ? (
              <input
                className="host-input min-w-0 flex-1 rounded-md border p-1"
                aria-label={translate('settings.dshCommandArguments', { value1: command.name })}
                placeholder={command.hint}
                value={args}
                disabled={disabled}
                onChange={(event) => setArgs(event.target.value)}
              />
            ) : (
              <span className="flex-1" />
            )}
            <button
              type="submit"
              className="host-button rounded-md border px-2 py-0.5"
              aria-label={translate('settings.dshRunCommand', { value1: command.name })}
              disabled={disabled}
            >
              {translate('settings.dshRun')}
            </button>
            {error && <ErrorMark message={error} />}
          </>
        )}
      </form>
      {command.description && (
        <p className="text-[var(--theme-fg-muted)]">{command.description}</p>
      )}
      {UPLOAD_COMMANDS.has(command.name) && (
        <p className="text-[var(--status-warning-fg)]">{translate('settings.dshFeedbackNotice')}</p>
      )}
    </li>
  );
}

/** A failed action, shown as a mark after the control that ran it. */
type Failure = { at: DshPanelAction['kind']; target?: string; message: string };
function failureTarget(action: DshPanelAction) {
  return action.kind === 'command' ? action.line.slice(1).split(/\s/)[0]
    : action.kind === 'setPluginEnabled' ? action.id
      : action.kind === 'setBundleEnabled' ? action.name
        : action.kind === 'updateSetting' ? `${action.ns}.${action.key}` : undefined;
}

const SELECTS: Record<string, (info: DshHarnessInfo) => string[]> = {
  'permission.defaultPreset': (info) => info.permissionPresets,
  'llm-deepseek.reasoningEffort': () => ['off', 'low', 'high', 'max'],
};

export function DshHarnessPanel({
  info: initial,
  readOnly,
  runAction,
  consoleUrl,
}: {
  info: DshHarnessInfo;
  readOnly: boolean;
  runAction: (action: DshPanelAction) => Promise<DshPanelResult>;
  /** Browser address for the console's loopback port; absent where it cannot open. */
  consoleUrl?: ((target: DshConsoleTarget) => Promise<string>) | undefined;
}) {
  useI18n();
  const [info, setInfo] = useState(initial);
  const [settings, setSettings] = useState<DshSettingsView[] | null>(null);
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState(false);
  const [restartRequired, setRestartRequired] = useState(false);
  // The live inventory reports the running process; saved toggles load after reconnect.
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const [failure, setFailure] = useState<Failure | null>(null);
  const [commandOutput, setCommandOutput] = useState<string | null>(null);
  // Uncontrolled setting inputs remount to show the stored value after a refusal.
  const [formKey, setFormKey] = useState(0);
  const run = async (action: DshPanelAction, keepError = false) => {
    // Toggles show the requested state at once and roll back on failure.
    const toggle = action.kind === 'setPluginEnabled' ? `plugin:${action.id}`
      : action.kind === 'setBundleEnabled' ? `bundle:${action.name}` : null;
    const previous = toggle ? pending[toggle] : undefined;
    const settle = (value: boolean | undefined) => {
      if (!toggle) return;
      setPending((current) => {
        const next = { ...current };
        if (value === undefined) delete next[toggle];
        else next[toggle] = value;
        return next;
      });
    };
    if (toggle && 'enabled' in action) settle(action.enabled);
    setBusy(true);
    if (!keepError) setFailure(null);
    try {
      const response = await runAction(action);
      if (response.harness) setInfo(response.harness);
      if (response.settings) setSettings(response.settings);
      if (response.result?.application === 'restart-required') setRestartRequired(true);
      else if (toggle) settle(previous);
      return response;
    } catch (failure) {
      settle(previous);
      const target = failureTarget(action);
      setFailure({
        at: action.kind,
        ...(target ? { target } : {}),
        message: failure instanceof Error ? failure.message : String(failure),
      });
      return null;
    } finally {
      setBusy(false);
    }
  };
  const updateSetting = async (
    view: DshSettingsView,
    key: string,
    value: SettingValue,
  ) => {
    const response = await run({
      kind: 'updateSetting',
      ns: view.ns,
      key,
      value,
      revision: view.revision,
    });
    const updated = response?.result as DshSettingsView | null | undefined;
    if (updated?.ns) {
      setSettings((current) =>
        current?.map((entry) => (entry.ns === updated.ns ? updated : entry)) ??
        null,
      );
      return;
    }
    // Refused or conflicting: reload stored values and keep the reason visible.
    await run({ kind: 'settings' }, true);
    setFormKey((key) => key + 1);
  };
  const reconnect = async () => {
    if (!(await run({ kind: 'restart' }))) return;
    const refreshed = await run({ kind: 'refresh' });
    if (!refreshed?.harness) return;
    const live = refreshed.harness;
    // Keep any toggle the new process still does not reflect.
    const remaining = Object.fromEntries(
      Object.entries(pending).filter(([key, enabled]) => {
        const split = key.indexOf(':');
        const id = key.slice(split + 1);
        const entry = key.slice(0, split) === 'plugin'
          ? live.plugins.find((plugin) => plugin.id === id)
          : live.bundles.find((bundle) => bundle.name === id);
        return entry?.enabled !== enabled;
      }),
    );
    setPending(remaining);
    setRestartRequired(Object.keys(remaining).length > 0);
  };
  const runCommand = async (line: string) => {
    setCommandOutput(null);
    const response = await run({ kind: 'command', line });
    if (!response) return;
    const name = line.slice(1).split(/\s/)[0];
    setCommandOutput(response.result?.result?.text?.trim()
      || translate('settings.dshCommandDone', { value1: name }));
  };
  const openConsole = (resolve: NonNullable<typeof consoleUrl>) => {
    // Reserve the tab inside the click so the browser does not block it as a
    // popup, and never hand the new page this window as its opener.
    const tab = window.open('about:blank', '_blank');
    if (!tab) {
      setFailure({ at: 'console', message: translate('devices.allowPopupsForThisSiteThenTry') });
      return;
    }
    tab.opener = null;
    void (async () => {
      const target = (await run({ kind: 'console' }))?.console;
      const url = target
        ? await resolve(target).catch((cause: unknown) => {
            setFailure({ at: 'console', message: cause instanceof Error ? cause.message : String(cause) });
            return null;
          })
        : null;
      if (url) tab.location.replace(url);
      else tab.close();
    })();
  };
  const disabled = readOnly || busy;
  const projections = info.session?.projections ?? {};
  const plan = projections.plan;
  const goal = projections.goal;
  const query = filter.toLowerCase();
  const runModes = info.runModes ?? [];
  const runMode = projections.agentPreset
    ?? runModes.find((mode) => mode.isDefault)?.id ?? '';
  const runModeLocked = info.session?.presetLocked !== false;
  const commands = info.session?.commands ?? [];
  const markFor = (at: Failure['at'][], target?: string) =>
    failure && at.includes(failure.at) && (target === undefined || failure.target === target)
      ? <ErrorMark message={failure.message} />
      : null;
  // Where each failure's control is on screen now; one whose control has gone
  // (the run stopped, a filtered row) shows its mark by the title instead.
  const anchored = !failure ? true
    : failure.at === 'stop' ? Boolean(info.session?.running)
      : failure.at === 'selectRunMode' ? runModes.length > 0
        : failure.at === 'command' ? commands.some((command) => command.name === failure.target)
          : failure.at === 'console' ? Boolean(info.features?.console && consoleUrl)
            : failure.at === 'restart' || failure.at === 'refresh' ? restartRequired
              : failure.at === 'setPluginEnabled'
                ? info.plugins.some((plugin) => plugin.id === failure.target
                  && `${plugin.name} ${plugin.module}`.toLowerCase().includes(query))
                : failure.at === 'setBundleEnabled' ? info.bundles.some((bundle) => bundle.name === failure.target)
                  : failure.at === 'updateSetting'
                    ? Boolean(settings?.some((view) => view.fields.some((field) => `${view.ns}.${field.key}` === failure.target)))
                    // Its mark replaces "Loading…" only while nothing was read.
                    : failure.at === 'settings' ? !settings
                      : true;
  return (
    <section aria-label={translate('settings.harnessPlugins')} className="space-y-4">
      <div>
        <div className="flex items-center gap-1">
          <h3 className="font-semibold">
            {translate('settings.dshTitle', {
              value1: info.version ?? '',
              value2: info.profile,
            })}
          </h3>
          {failure && !anchored && <ErrorMark message={failure.message} />}
        </div>
        <p className="text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.dshBridgeNotice')}
        </p>
      </div>
      {info.session?.running && (
        <div role="status" className="flex items-center gap-2 rounded-md border p-2">
          <span className="flex-1">{translate('settings.dshAutonomous')}</span>
          <button type="button" className="host-button rounded-md border px-2 py-1"
            disabled={disabled} onClick={() => void run({ kind: 'stop' })}>
            {translate('settings.dshStop')}
          </button>
          {markFor(['stop'])}
        </div>
      )}
      <section aria-label={translate('settings.dshSession')} className="space-y-1">
        <h4 className="font-semibold">{translate('settings.dshSession')}</h4>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
          <dt>{translate('settings.dshPermissionPreset')}</dt>
          <dd>{projections.permissions?.currentValue ?? translate('settings.dshNone')}</dd>
          <dt>{translate('settings.dshPlanMode')}</dt>
          <dd>
            {translate(plan?.active ? 'settings.dshOn' : 'settings.dshOff')}
            {plan?.pending ? ` · ${translate('settings.dshPending')}` : ''}
          </dd>
          <dt>{translate('settings.dshGoal')}</dt>
          <dd className="break-words">
            {goal
              ? translate('settings.dshGoalState', {
                  value1: goal.goal.objective,
                  value2: goal.goal.phase,
                  value3: String(goal.roundsStarted),
                })
              : translate('settings.dshNone')}
          </dd>
        </dl>
        {!!projections.todos?.length && (
          <ul aria-label={translate('settings.dshTodos')} className="space-y-0.5 text-xs">
            {projections.todos.map((todo, index) => (
              <li key={index}>
                {todo.status === 'completed' ? '☑' : todo.status === 'in_progress' ? '◐' : '☐'}{' '}
                {todo.content}
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-label={translate('settings.dshRunMode')} className="space-y-1">
        <h4 className="font-semibold">{translate('settings.dshRunMode')}</h4>
        {runModes.length > 0 ? (
          <>
            <div className="flex items-center gap-1">
              <select className="host-input w-full min-w-0 flex-1 rounded-md border p-1"
                aria-label={translate('settings.dshRunMode')} value={runMode}
                disabled={disabled || runModeLocked}
                onChange={(event) => void run({ kind: 'selectRunMode', id: event.target.value })}>
                {runModes.map((mode) => (
                  <option key={mode.id} value={mode.id} disabled={Boolean(mode.broken)}>{dshRunModeName(mode)}</option>
                ))}
              </select>
              {markFor(['selectRunMode'])}
            </div>
            <p className="text-xs text-[var(--theme-fg-muted)]">
              {runModeLocked
                ? translate('settings.dshRunModeLocked')
                : dshRunModeHint(runModes.find((mode) => mode.id === runMode))}
            </p>
          </>
        ) : (
          <p className="text-xs text-[var(--theme-fg-muted)]">
            {translate('settings.dshRunModeUnavailable')}
            {info.compositionError ? (
              <>
                {' '}
                <ErrorMark kind="unavailable" message={info.compositionError} />
              </>
            ) : null}
          </p>
        )}
      </section>
      {commands.length > 0 && (
        <section aria-label={translate('settings.dshCommands')} className="space-y-2">
          <h4 className="font-semibold">{translate('settings.dshCommands')}</h4>
          <p className="text-xs text-[var(--theme-fg-muted)]">
            {translate('settings.dshCommandsNotice')}
          </p>
          <ul className="max-h-64 space-y-2 overflow-y-auto overscroll-contain">
            {commands.map((command) => (
              <DshCommandRow key={command.name} command={command} disabled={disabled}
                error={failure?.at === 'command' && failure.target === command.name ? failure.message : null}
                onRun={(line) => void runCommand(line)} />
            ))}
          </ul>
          {commandOutput && (
            <pre role="status" className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border p-2 text-xs">
              {commandOutput}
            </pre>
          )}
        </section>
      )}
      {info.features?.console && consoleUrl && (
        <section aria-label={translate('settings.dshConsole')} className="space-y-1">
          <h4 className="font-semibold">{translate('settings.dshConsole')}</h4>
          <p className="text-xs text-[var(--theme-fg-muted)]">
            {translate('settings.dshConsoleNotice')}
          </p>
          <div className="flex items-center gap-1">
            <button type="button" className="host-button rounded-md border px-2 py-1"
              disabled={disabled} onClick={() => openConsole(consoleUrl)}>
              {translate('settings.dshOpenConsole')}
            </button>
            {markFor(['console'])}
          </div>
        </section>
      )}
      <details
        onToggle={(event) => {
          if ((event.target as HTMLDetailsElement).open && !settings)
            void run({ kind: 'settings' });
        }}
      >
        <summary className="cursor-pointer font-semibold">
          {translate('settings.dshProfileSettings')}
        </summary>
        <p className="my-1 text-xs text-[var(--theme-fg-muted)]">
          {translate('settings.dshProfileSettingsNotice')}
        </p>
        {settings ? (
          <div className="space-y-2" key={formKey}>
            {settings.flatMap((view) =>
              view.fields.map((field) => {
                const id = `${view.ns}.${field.key}`;
                const choices = SELECTS[id]?.(info);
                const label = (
                  <span className="font-mono text-xs">
                    {id}
                    {field.overridden && ` · ${translate('settings.dshCustomized')}`}
                    {failure?.at === 'updateSetting' && failure.target === id ? <> {markFor(['updateSetting'], id)}</> : null}
                  </span>
                );
                if (field.type === 'boolean')
                  return (
                    <label key={id} className="flex items-center gap-2">
                      <input type="checkbox" aria-label={id} checked={field.value === true}
                        disabled={disabled}
                        onChange={(event) => void updateSetting(view, field.key, event.target.checked)} />
                      {label}
                    </label>
                  );
                if (choices)
                  return (
                    <label key={id} className="block">
                      {label}
                      <select className="host-input mt-1 w-full rounded-md border p-1" aria-label={id}
                        value={field.value === null ? '' : String(field.value)} disabled={disabled}
                        onChange={(event) =>
                          void updateSetting(view, field.key, event.target.value || null)
                        }>
                        <option value="">{translate('settings.dshDefault')}</option>
                        {choices.map((choice) => (
                          <option key={choice} value={choice}>{choice}</option>
                        ))}
                      </select>
                    </label>
                  );
                return (
                  <label key={id} className="block">
                    {label}
                    <input className="host-input mt-1 w-full rounded-md border p-1" aria-label={id}
                      type={field.type === 'number' ? 'number' : 'text'}
                      defaultValue={field.value === null ? '' : String(field.value)}
                      disabled={disabled}
                      onBlur={(event) => {
                        const raw = event.target.value.trim();
                        const value = raw === '' ? null
                          : field.type === 'number' ? Number(raw) : raw;
                        if (value !== field.value) void updateSetting(view, field.key, value);
                      }} />
                  </label>
                );
              }),
            )}
          </div>
        ) : failure?.at === 'settings' ? (
          // The stored values never arrived: the reason and a Retry on the mark.
          <p className="flex items-center gap-1 text-xs">
            <ErrorMark message={failure.message} onRetry={() => void run({ kind: 'settings' })} />
          </p>
        ) : (
          <p className="text-xs">{translate('settings.loading')}</p>
        )}
      </details>
      {restartRequired && (
        <div role="status" className="flex items-center gap-2 rounded-md border p-2 text-xs">
          <span className="flex-1">{translate('settings.dshRestartRequired')}</span>
          <button type="button" className="host-button rounded-md border px-2 py-1"
            disabled={disabled} onClick={() => void reconnect()}>
            {translate('settings.dshReconnect')}
          </button>
          {markFor(['restart', 'refresh'])}
        </div>
      )}
      <section aria-label={translate('settings.plugins')} className="space-y-2">
        <h4 className="font-semibold">
          {translate('settings.plugins')} {info.profile} {translate('settings.profile')}
        </h4>
        <input className="host-input w-full rounded-md border p-2"
          aria-label={translate('settings.filterPlugins')}
          placeholder={translate('settings.filterPlugins')} value={filter}
          onChange={(event) => setFilter(event.target.value)} />
        <ul className="max-h-56 space-y-1 overflow-y-auto overscroll-contain">
          {info.plugins
            .filter((plugin) =>
              `${plugin.name} ${plugin.module}`.toLowerCase().includes(query),
            )
            .map((plugin) => {
              const saved = pending[`plugin:${plugin.id}`];
              const enabled = saved ?? plugin.enabled;
              return (
                <li className="flex items-center gap-2 break-all text-xs" key={plugin.id}
                  title={plugin.description}>
                  <input type="checkbox"
                    aria-label={translate('settings.dshEnable', { value1: plugin.name })}
                    checked={enabled}
                    disabled={disabled || Boolean(plugin.readOnly)}
                    onChange={(event) =>
                      void run({ kind: 'setPluginEnabled', id: plugin.id, enabled: event.target.checked })
                    } />
                  <span className="flex-1">{plugin.name}</span>
                  <span className="text-[var(--theme-fg-muted)]">
                    {plugin.readOnly
                      ? translate('settings.dshManaged')
                      : translate(enabled ? 'settings.enabled' : 'settings.disabled')}
                    {saved !== undefined && saved !== plugin.enabled
                      ? ` · ${translate('settings.dshAfterReconnect')}`
                      : ''}
                  </span>
                  {markFor(['setPluginEnabled'], plugin.id)}
                </li>
              );
            })}
        </ul>
      </section>
      <section aria-label={translate('settings.dshBundles')} className="space-y-1">
        <h4 className="font-semibold">{translate('settings.dshBundles')}</h4>
        <ul className="space-y-1 text-xs">
          {info.bundles.map((bundle) => {
            const saved = pending[`bundle:${bundle.name}`];
            return (
              <li className="flex items-center gap-2 break-all" key={bundle.name} title={bundle.description}>
                <input type="checkbox"
                  aria-label={translate('settings.dshEnable', { value1: bundle.name })}
                  checked={saved ?? bundle.enabled} disabled={disabled || Boolean(bundle.readOnly)}
                  onChange={(event) =>
                    void run({ kind: 'setBundleEnabled', name: bundle.name, enabled: event.target.checked })
                  } />
                <span className="flex-1">{bundle.name}</span>
                <span className="text-[var(--theme-fg-muted)]">
                  {bundle.version}
                  {saved !== undefined && saved !== bundle.enabled
                    ? ` · ${translate('settings.dshAfterReconnect')}`
                    : ''}
                </span>
                {markFor(['setBundleEnabled'], bundle.name)}
              </li>
            );
          })}
        </ul>
      </section>
      <section aria-label={translate('settings.dshProviders')} className="space-y-1">
        <h4 className="font-semibold">{translate('settings.dshProviders')}</h4>
        <ul className="max-h-32 space-y-0.5 overflow-y-auto text-xs">
          {info.providers.map((provider) => (
            <li key={provider.id} className="flex gap-2">
              <span className="flex-1">{provider.name}</span>
              {provider.declared && (
                <span className="text-[var(--theme-fg-muted)]">{translate('settings.dshInProfile')}</span>
              )}
            </li>
          ))}
        </ul>
      </section>
    </section>
  );
}

/**
 * The DeepSeek Harness plugin's workbench panel: the thread's live DSH
 * controls without the generic harness fields. Owners load the live session
 * (starting it if needed); viewers get the capability snapshot.
 */
export function DshPluginPanel({
  loadCapabilities,
  readOnly,
  runHarnessAction,
  dshConsoleUrl,
}: {
  loadCapabilities: () => Promise<{ negotiated?: unknown }>;
  readOnly: boolean;
  runHarnessAction: (action: DshPanelAction) => Promise<DshPanelResult>;
  dshConsoleUrl?: ((target: DshConsoleTarget) => Promise<string>) | undefined;
}) {
  useI18n();
  const [info, setInfo] = useState<DshHarnessInfo | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const snapshot = await loadCapabilities();
      const harness = (snapshot.negotiated as { harness?: unknown } | null)?.harness;
      if (isDshHarness(harness) || readOnly) return harness;
      // Not live yet: a harness action resumes the session first.
      return (await runHarnessAction({ kind: 'refresh' })).harness;
    })().then(
      (harness) => { if (!cancelled) setInfo(isDshHarness(harness) ? harness : null); },
      (failure: unknown) => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : String(failure));
      },
    );
    return () => { cancelled = true; };
  }, [loadCapabilities, readOnly, runHarnessAction]);
  // Nothing could be loaded: the reason on a mark, beside a short neutral line.
  if (error)
    return (
      <p className="flex items-center gap-1 p-3 text-sm">
        {translate('files.unavailable')}
        <ErrorMark message={error} />
      </p>
    );
  if (info === undefined)
    return <p className="p-3 text-sm">{translate('settings.loadingHarnessCapabilities')}</p>;
  if (!info)
    return <p className="p-3 text-sm">{translate('workbench.deepseekHarnessNotRunning')}</p>;
  return (
    <div className="h-full overflow-y-auto p-3 text-sm" data-testid="dsh-plugin-panel">
      <DshHarnessPanel info={info} readOnly={readOnly} runAction={runHarnessAction}
        consoleUrl={dshConsoleUrl} />
    </div>
  );
}
