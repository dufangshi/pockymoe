import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError } from '../../lib/api';
import { fetchEnvironments, type EnvironmentsDto, type EnvironmentState } from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';

export const GIB = 1024 ** 3;
export const environmentTransitioning = (state: EnvironmentState) => ['creating', 'starting', 'stopping'].includes(state);
export function environmentError(cause: unknown, fallback: EnvironmentMessage = 'operationFailed') {
  if (cause instanceof ApiError) {
    const code = String(cause.payload.code), message = String(cause.payload.message ?? '');
    if (code === 'environment_owner_authorization_required') return t('ownerRequired');
    if ([404, 501].includes(cause.statusCode)) return t('unsupported');
    if ([401, 403].includes(cause.statusCode)) return t('forbidden');
    if (code.includes('budget') || message.includes('budget would be exceeded')) return t('budgetExceeded');
    // Known Supervisor refusals, matched on their stable prefix; the raw text is never shown.
    if (message.includes('thread already owns an environment')) return t('alreadyOwns');
    if (message.includes('close existing child threads')) return t('closeChildren');
    if (message.includes('stop the active turn')) return t('threadBusy');
    if (cause.statusCode === 409) return t('conflict');
  }
  // Never render upstream HTTP bodies or an error containing the submitted key.
  return t(fallback);
}
export function positive(value: string, integer = false, zero = false) {
  const number = value.trim() ? Number(value) : NaN;
  return Number.isFinite(number) && (zero ? number >= 0 : number > 0) && (!integer || Number.isSafeInteger(number));
}
export function useEnvironmentSnapshot(deviceId?: string | null) {
  const [snapshot, setSnapshot] = useState<EnvironmentsDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const scope = useRef(0);
  const refresh = useCallback(async (manual = true) => {
    const version = scope.current;
    if (manual) { setLoading(true); setError(null); }
    try {
      const next = await fetchEnvironments(deviceId);
      if (scope.current === version) { setSnapshot(next); if (manual) setError(null); }
    } catch (cause) {
      if (scope.current === version && manual) setError(environmentError(cause, 'loadFailed'));
    } finally { if (scope.current === version && manual) setLoading(false); }
  }, [deviceId]);
  useEffect(() => {
    scope.current += 1;
    setSnapshot(null);
    void refresh();
    return () => { scope.current += 1; };
  }, [refresh]);
  const active = snapshot?.environments.some(env => environmentTransitioning(env.state) || env.state === 'running');
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => { if (document.visibilityState !== 'hidden') void refresh(false); }, 5000);
    return () => window.clearInterval(timer);
  }, [active, refresh]);
  return { snapshot, setSnapshot, loading, error, refresh };
}

export const ENVIRONMENT_STATES = ['creating', 'stopped', 'starting', 'running', 'stopping', 'error', 'destroyed', 'interrupted'] as const;
export const environmentStateKey = (state: string): EnvironmentMessage =>
  (ENVIRONMENT_STATES as readonly string[]).includes(state) ? state as EnvironmentMessage : 'unknown';
/** Lifecycle actions offered for a state; the server still decides. */
export function environmentActions(state: EnvironmentState): ('start' | 'stop' | 'rebuild' | 'destroy')[] {
  if (state === 'destroyed' || environmentTransitioning(state)) return [];
  if (state === 'running') return ['stop', 'rebuild', 'destroy'];
  // A failed generation is not safely startable: the Supervisor asks for Stop
  // (or Rebuild) first, so that is what it offers.
  if (state === 'error') return ['stop', 'rebuild', 'destroy'];
  // Interrupted still counts toward the budget until stopped; Start resumes it.
  if (state === 'interrupted') return ['start', 'stop', 'rebuild', 'destroy'];
  return ['start', 'rebuild', 'destroy'];
}
/** Still holds a container or a budget reservation. */
export const environmentHoldsResources = (state: EnvironmentState) => ['running', 'starting', 'creating', 'stopping', 'interrupted', 'error'].includes(state);
/** Why an approved migration failed, in the reader's words, from the Supervisor's stable reason text. */
export function migrationFailure(lastError: string | null | undefined): EnvironmentMessage {
  const text = lastError ?? '';
  if (text.includes('budget would be exceeded')) return 'failedBudget';
  if (text.includes('not safely startable')) return 'failedNotStartable';
  return 'failedOther';
}
export const gibValue = (bytes: number) => Math.round((bytes / GIB) * 100) / 100;
