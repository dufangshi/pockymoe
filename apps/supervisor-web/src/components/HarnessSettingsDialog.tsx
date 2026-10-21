import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AgentCapabilitySnapshotDto,
  ModelOptionDto,
  ThreadDto,
} from '@pockymoe/shared';
import { fetchThreadCapabilitySnapshot, postThreadHarnessAction } from '../lib/api';
import {
  DshHarnessPanel,
  isDshHarness,
  type DshPanelAction,
  type DshConsoleTarget,
  type DshPanelResult,
} from './DshHarnessPanel';
import { FormDialog } from './FormDialog';

export interface HarnessSettingsFieldsProps {
  thread: ThreadDto;
  models: ModelOptionDto[];
  busy: boolean;
  readOnly?: boolean;
  onChange: (input: {
    model?: string;
    reasoningEffort?: ThreadDto['reasoningEffort'];
    sandboxMode?: Exclude<ThreadDto['sandboxMode'], undefined>;
  }) => Promise<void>;
  loadCapabilities?: () => Promise<AgentCapabilitySnapshotDto>;
  /** Device-scoped typed panel action; defaults to the local supervisor. */
  runHarnessAction?: (action: DshPanelAction) => Promise<DshPanelResult>;
  /** Harness panel actions are owner-only on Relay, even for thread controllers. */
  harnessReadOnly?: boolean;
  /** Browser address of the native DSH console for this thread's device. */
  dshConsoleUrl?: ((target: DshConsoleTarget) => Promise<string>) | undefined;
  /** The fields came into view: the host may refresh the model list. */
  onOpen?: () => void;
  /** A model or effort changed during a turn applies from its next model call;
      otherwise from the next turn. */
  liveModelSwitch?: boolean;
}

export function HarnessSettingsDialog({
  onClose,
  composerFocusOwner,
  ...fields
}: HarnessSettingsFieldsProps & {
  onClose: () => void;
  composerFocusOwner?: string;
}) {
  // Radix may mount portal children after the wrapper's layout effects.
  // Mark the surface from the ref callback before dialog autofocus runs.
  const attachFocusOwner = useCallback(
    (node: HTMLDivElement | null) => {
      const dialog = node?.closest<HTMLElement>('[role="dialog"]');
      if (!dialog) return;
      if (composerFocusOwner)
        dialog.dataset.composerFocusOwner = composerFocusOwner;
      else delete dialog.dataset.composerFocusOwner;
    },
    [composerFocusOwner],
  );
  return (
    <FormDialog title={translate('settings.harnessSettings')} onClose={onClose}>
      <div
        ref={attachFocusOwner}
        data-composer-focus-owner={composerFocusOwner}
      >
        <HarnessSettingsFields {...fields} />
      </div>
    </FormDialog>
  );
}

// Adapter data is metadata only. Never execute backend-provided HTML or JavaScript.
export function HarnessSettingsFields({
  thread,
  models,
  busy,
  readOnly = false,
  onChange,
  loadCapabilities,
  runHarnessAction,
  dshConsoleUrl,
  harnessReadOnly = false,
  onOpen,
  liveModelSwitch = false,
}: HarnessSettingsFieldsProps) {
  useI18n();
  const opened = useRef(onOpen);
  opened.current = onOpen;
  useEffect(() => {
    opened.current?.();
  }, [thread.id]);
  const [info, setInfo] = useState<{
    profile?: string;
    notice?: string;
    plugins?: { id: string; name: string; enabled: boolean }[];
  } | null>(null);
  const harnessAction = runHarnessAction
    ?? ((action: DshPanelAction) => postThreadHarnessAction<DshPanelResult>(thread.id, action));
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [filter, setFilter] = useState('');
  useEffect(() => {
    let cancelled = false;
    setInfo(null);
    setError(null);
    (loadCapabilities
      ? loadCapabilities()
      : fetchThreadCapabilitySnapshot(thread.id)
    )
      .then((snapshot) => {
        if (!cancelled)
          setInfo(
            (snapshot.negotiated as { harness?: typeof info } | null)
              ?.harness ?? null,
          );
      })
      .catch((error) => {
        if (!cancelled) setError(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [thread.id, loadCapabilities, attempt]);
  const model = models.find((model) => model.model === thread.model);
  // A running turn does not lock them: the harness takes a change mid-turn.
  const disabled = readOnly || busy;
  return (
    <div className="space-y-4 text-sm">
      <label className="block">
        {translate('settings.model')}
        <select
          className="host-input mt-1 w-full rounded-md border p-2"
          aria-label={translate('settings.harnessModel')}
          value={thread.model ?? ''}
          disabled={disabled}
          onChange={(event) => {
            const selected = models.find(
              (model) => model.model === event.target.value,
            );
            void onChange({
              model: event.target.value,
              reasoningEffort: selected?.defaultReasoningEffort ?? null,
            });
          }}
        >
          {models.map((model) => (
            <option key={model.id} value={model.model}>
              {model.displayName}
            </option>
          ))}
        </select>
      </label>
      {!!model?.supportedReasoningEfforts.length && (
        <label className="block">
          {translate('settings.reasoningEffort')}
          <select
            className="host-input mt-1 w-full rounded-md border p-2"
            aria-label={translate('settings.harnessReasoningEffort')}
            value={thread.reasoningEffort ?? ''}
            disabled={disabled}
            onChange={(event) =>
              void onChange({ reasoningEffort: event.target.value })
            }
          >
            {model.supportedReasoningEfforts.map((effort) => (
              <option
                key={effort.reasoningEffort}
                value={effort.reasoningEffort}
              >
                {effort.reasoningEffort ||
                  translate('settings.providerDefault')}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="block">
        {translate('settings.workspacePermissions')}
        <select
          className="host-input mt-1 w-full rounded-md border p-2"
          aria-label={translate('settings.workspacePermissions')}
          value={thread.sandboxMode ?? 'danger-full-access'}
          disabled={disabled}
          onChange={(event) =>
            void onChange({
              sandboxMode: event.target.value as Exclude<ThreadDto['sandboxMode'], undefined>,
            })
          }
        >
          <option value="danger-full-access">
            {translate('settings.fullAccess')}
          </option>
          <option value="workspace-write">
            {translate('chat.workspaceWrite')}
          </option>
          <option value="read-only">{translate('chat.readOnly')}</option>
        </select>
      </label>
      {thread.activeTurnId && !readOnly && (
        <p className="settings-hint text-[var(--theme-fg-muted)]">
          {translate(liveModelSwitch ? 'settings.sessionSettingsApplyNextCall' : 'settings.sessionSettingsApplyNextTurn')}
        </p>
      )}
      {isDshHarness(info) ? (
        <DshHarnessPanel info={info} readOnly={readOnly || harnessReadOnly} runAction={harnessAction}
          consoleUrl={dshConsoleUrl} />
      ) : error ? (
        // The capabilities never arrived: a short line, the reason and Retry on its mark.
        <p className="flex items-center gap-1 text-[var(--theme-fg-muted)]">
          {translate('files.unavailable')}
          <ErrorMark message={error} onRetry={() => setAttempt((value) => value + 1)} />
        </p>
      ) : (
      <p className="text-[var(--theme-fg-muted)]">
        {info?.notice ?? translate('settings.loadingHarnessCapabilities')}
      </p>
      )}
      {info && !isDshHarness(info) && (
        <section
          aria-label={translate('settings.harnessPlugins')}
          className="space-y-2"
        >
          <h3 className="font-semibold">
            {translate('settings.plugins')} {info.profile}{' '}
            {translate('settings.profile')}
          </h3>
          <p className="text-xs text-[var(--theme-fg-muted)]">
            {translate('settings.readOnlySnapshotFromThisSessionS')}
          </p>
          <input
            className="host-input w-full rounded-md border p-2"
            aria-label={translate('settings.filterPlugins')}
            placeholder={translate('settings.filterPlugins')}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          <ul className="max-h-56 space-y-1 overflow-y-auto overscroll-contain">
            {info.plugins
              ?.filter((plugin) =>
                plugin.name?.toLowerCase().includes(filter.toLowerCase()),
              )
              .map((plugin) => (
                <li className="flex gap-2 break-all text-xs" key={plugin.id}>
                  <span>{plugin.enabled ? '●' : '○'}</span>
                  <span>{plugin.name}</span>
                  <span className="ml-auto">
                    {plugin.enabled
                      ? translate('settings.enabled')
                      : translate('settings.disabled')}
                  </span>
                </li>
              ))}
          </ul>
        </section>
      )}
    </div>
  );
}
