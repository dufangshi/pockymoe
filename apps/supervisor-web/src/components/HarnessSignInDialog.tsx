import { useEffect, useRef, useState } from 'react';
import { ErrorMark, SingleShellTerminal } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { ShellSessionDto } from '@pockymoe/shared';
import { connectShellSocket, request } from '../lib/api';
import { FormDialog } from './FormDialog';

/** A device's API root: this Supervisor (`null`) or a relay device. */
export function deviceApiRoot(deviceId: string | null) {
  return deviceId ? `relay/devices/${encodeURIComponent(deviceId)}/api` : 'api';
}

type SignInJob = { state?: string; action?: string; signedIn?: boolean; message?: string; error?: string };
type Phase =
  | { at: 'starting' }
  | { at: 'failed'; message: string }
  | { at: 'running'; shell: ShellSessionDto; command: string }
  | { at: 'checking'; shell: ShellSessionDto; command: string }
  | { at: 'checked'; shell: ShellSessionDto; command: string; job: SignInJob };

function effectiveTheme(): 'light' | 'dark' {
  const root = document.documentElement;
  return root.dataset.themeEffective === 'dark' || root.classList.contains('dark') ? 'dark' : 'light';
}

/**
 * Sign in to a harness from the Web (issue #178): the window is a terminal on
 * the device that runs the harness's own sign-in command from the device's
 * catalog. The device checks the harness again when the command ends; closing
 * the window ends a sign-in still running. `onClose` runs after either, so the
 * opener reloads the harness's state.
 */
export function HarnessSignInDialog({
  deviceId,
  agentId,
  name,
  onClose,
}: {
  deviceId: string | null;
  agentId: string;
  name: string;
  onClose: () => void;
}) {
  useI18n();
  const apiRoot = deviceApiRoot(deviceId);
  const [phase, setPhase] = useState<Phase>({ at: 'starting' });
  const live = useRef<string | null>(null);
  const theme = useRef(effectiveTheme());

  type Started = { shell: ShellSessionDto; command: string };
  // One sign-in per window, even when React runs this effect twice (Strict
  // Mode): a second request would replace the first terminal on the device.
  const started = useRef<Promise<Started> | null>(null);
  const mounted = useRef(false);
  // The command ended, or Close already ended it.
  const finished = useRef(false);
  useEffect(() => {
    mounted.current = true;
    started.current ??= request<Started>(`${apiRoot}/shells/sign-in`, {
      method: 'POST',
      body: JSON.stringify({ agentId }),
    });
    let active = true;
    started.current.then(({ shell, command }) => {
      if (!active) return;
      live.current = shell.id;
      setPhase(current => current.at === 'starting' ? { at: 'running', shell, command } : current);
    }, (error: unknown) => {
      if (active) setPhase({ at: 'failed', message: error instanceof Error ? error.message : translate('devices.signInUnavailable') });
    });
    return () => {
      active = false;
      mounted.current = false;
      // Leaving the page ends the sign-in like closing the window; a Strict
      // Mode remount sets `mounted` again before this runs.
      const pending = started.current;
      window.setTimeout(() => {
        if (mounted.current || !pending || finished.current) return;
        void pending.then(({ shell }) => terminate(shell.id), () => undefined);
      }, 0);
    };
  }, [apiRoot, agentId]);

  // The device records the check as the harness's `signIn` job.
  useEffect(() => {
    if (phase.at !== 'checking') return;
    let stopped = false;
    const poll = async () => {
      try {
        const jobs = await request<Record<string, SignInJob>>(`${apiRoot}/management/jobs`, { cache: 'no-store' });
        const job = jobs[agentId];
        if (!stopped && job?.action === 'signIn' && job.state !== 'running') {
          setPhase(current => current.at === 'checking' ? { ...current, at: 'checked', job } : current);
          return;
        }
      } catch {
        /* Keep asking; the window still closes normally. */
      }
      if (!stopped) timer = window.setTimeout(() => void poll(), 1000);
    };
    let timer = window.setTimeout(() => void poll(), 300);
    return () => { stopped = true; window.clearTimeout(timer); };
  }, [phase.at, apiRoot, agentId]);

  function terminate(id: string) {
    return request(`${apiRoot}/shells/${encodeURIComponent(id)}/terminate`, { method: 'POST' }).catch(() => undefined);
  }
  function close() {
    finished.current = phase.at !== 'starting';
    if (live.current) void terminate(live.current);
    live.current = null;
    onClose();
  }

  const command = 'command' in phase ? phase.command : null;
  const result = phase.at === 'checked'
    ? phase.job.state === 'failed'
      ? { tone: 'failed' as const, text: translate('devices.signInCheckFailed', { value1: phase.job.error ?? '' }) }
      : phase.job.signedIn
        ? { tone: 'done' as const, text: translate('devices.signedIn') }
        : { tone: 'pending' as const, text: translate('devices.notSignedInYet') }
    : null;
  return (
    <FormDialog
      title={translate('devices.signInTo', { value1: name })}
      {...(command ? { description: translate('devices.signInWindowDescription', { value1: command }) } : {})}
      className="harness-sign-in-dialog sm:!max-w-3xl"
      secondary
      onClose={close}
    >
      <div className="harness-sign-in-terminal flex h-[min(56dvh,24rem)] min-h-56 flex-col overflow-hidden rounded-lg border border-[var(--theme-border)]" data-testid="harness-sign-in-terminal">
        {phase.at === 'starting' && <p role="status" className="p-3 text-xs">{translate('devices.signInStarting')}</p>}
        {phase.at === 'failed' && (
          <p className="flex items-center gap-1 p-3 text-xs">
            {translate('devices.signInUnavailable')}<ErrorMark message={phase.message} />
          </p>
        )}
        {'shell' in phase && (
          <SingleShellTerminal
            shell={phase.shell}
            effectiveTheme={theme.current}
            shellAdapter={{ connectSocket: handlers => connectShellSocket(handlers, { deviceId, threadId: null }) }}
            onExit={() => {
              finished.current = true;
              live.current = null;
              setPhase(current => current.at === 'running' ? { ...current, at: 'checking' } : current);
            }}
          />
        )}
      </div>
      {(phase.at === 'checking' || result) && (
        <p role="status" className="mt-2 text-xs">
          {phase.at === 'checking'
            ? `${translate('devices.signInEnded')} ${translate('devices.checkingSignIn')}`
            : <span className={result!.tone === 'done' ? 'text-[var(--status-success-fg)]' : result!.tone === 'failed' ? 'text-[var(--status-danger-fg)]' : 'text-[var(--status-warning-fg)]'}>{result!.text}</span>}
          {result?.tone === 'pending' && phase.at === 'checked' && phase.job.message && (
            <span className="mt-1 block text-[var(--theme-fg-muted)]">{phase.job.message}</span>
          )}
        </p>
      )}
      <div className="form-dialog-actions">
        <button type="button" className="host-secondary-button min-h-11 rounded-md border px-4 text-sm" data-dialog-cancel onClick={close}>
          {translate('devices.close')}
        </button>
      </div>
    </FormDialog>
  );
}
