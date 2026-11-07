import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError } from '../../lib/api';
import { fetchEnvironments, type EnvironmentsDto, type EnvironmentState } from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';

export const GIB = 1024 ** 3;
export const environmentTransitioning = (state: EnvironmentState) => ['creating', 'starting', 'stopping'].includes(state);
export function environmentError(cause: unknown, fallback: EnvironmentMessage = 'operationFailed') {
  if (cause instanceof ApiError) {
    if ([404, 501].includes(cause.statusCode)) return t('unsupported');
    if ([401, 403].includes(cause.statusCode)) return t('forbidden');
    if (String(cause.payload.code).includes('budget')) return t('budgetExceeded');
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
  return state === 'running' ? ['stop', 'rebuild', 'destroy'] : ['start', 'rebuild', 'destroy'];
}
export const gibValue = (bytes: number) => Math.round((bytes / GIB) * 100) / 100;
