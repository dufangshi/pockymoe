import type { ReactNode } from 'react';
import { Check } from 'lucide-react';
import { formatNumber } from '@pockymoe/thread-ui/i18n';
import type { EnvironmentLimits, EnvironmentRequestDto, EnvironmentState } from '../../lib/environmentApi';
import { environmentText as t } from '../../lib/environmentI18n';
import { environmentRequestText as rt } from '../../lib/environmentRequestI18n';
import { environmentStateKey, gibValue } from './model';

/** Environment lifecycle mark (docs/ui-principles.md §6, «隔离环境状态»): its own
    shapes, never a thread status, always followed by the state's name. */
export function EnvironmentStateMark({ state }: { state: EnvironmentState | string }) {
  const key = environmentStateKey(state);
  return <span className="env-state" data-state={key}><span className="env-state-mark" aria-hidden="true" />{t(key)}</span>;
}

export function SectionHeading({ id, title, count, children }: { id?: string; title: string; count?: number | undefined; children?: ReactNode }) {
  return <header className="env-heading">
    <h3 id={id}>{title}{count !== undefined && count > 0 && <span className="env-heading-count">{formatNumber(count)}</span>}</h3>
    {children && <div className="env-heading-actions">{children}</div>}
  </header>;
}

/** One label/value line of a fact list; the value column takes the rest. */
export function Fact({ label, children, note }: { label: ReactNode; children: ReactNode; note?: ReactNode }) {
  return <div className="env-fact"><dt>{label}</dt><dd>{children}{note && <small className="env-note">{note}</small>}</dd></div>;
}

export function Tag({ tone = 'neutral', children }: { tone?: 'neutral' | 'warning' | 'success' | 'danger' | 'accent'; children: ReactNode }) {
  return <span className="env-tag" data-tone={tone}>{children}</span>;
}

export function limitsText(value: EnvironmentLimits) {
  return t('limitsLine', { cpu: formatNumber(value.cpu), memory: formatNumber(gibValue(value.memoryBytes)), pids: formatNumber(value.pids), nofile: formatNumber(value.nofile) });
}
export const sameLimits = (a: EnvironmentLimits, b: EnvironmentLimits) =>
  a.cpu === b.cpu && a.memoryBytes === b.memoryBytes && a.pids === b.pids && a.nofile === b.nofile;

/** Request states use the thread marks: waiting for you, working, failed. */
export function requestMark(state: EnvironmentRequestDto['state']) {
  return state === 'pending' ? 'waiting' : state === 'failed' ? 'failed' : state === 'waitingIdle' || state === 'provisioning' ? 'running' : 'idle';
}
export const requestActive = (state: EnvironmentRequestDto['state']) => ['pending', 'waitingIdle', 'provisioning', 'failed'].includes(state);

const STEPS = ['stepApprove', 'stepIdle', 'stepMigrate', 'stepReady'] as const;
/** Approve → wait for idle → migrate → isolated. Rejected or cancelled requests have no progress. */
export function MigrationSteps({ state }: { state: EnvironmentRequestDto['state'] }) {
  const current = { pending: 0, waitingIdle: 1, provisioning: 2, failed: 2, ready: 4 }[state as string];
  if (current === undefined) return null;
  return <ol className="env-steps" aria-label={rt('progress')}>
    {STEPS.map((step, index) => {
      const status = index < current ? 'done' : index === current ? (state === 'failed' ? 'failed' : 'current') : 'next';
      return <li key={step} data-step={status} aria-current={status === 'current' || status === 'failed' ? 'step' : undefined}>
        <span className="env-step-dot" aria-hidden="true">{status === 'done' && <Check />}</span>
        <span>{rt(step)}</span>
        {status !== 'next' && <span className="sr-only">{rt(status === 'done' ? 'stepDone' : status === 'failed' ? 'stepFailed' : 'stepCurrent')}</span>}
      </li>;
    })}
  </ol>;
}
