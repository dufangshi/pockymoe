import { useId, useState } from 'react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { saveEnvironmentBudget, type EnvironmentBudgetDto } from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';
import { EnvironmentWindow } from './EnvironmentWindow';
import { GIB, environmentError, positive } from './model';

export function BudgetEditor({ budget, deviceId, onClose, onSaved }: {
  budget: EnvironmentBudgetDto; deviceId: string | null; onClose(): void; onSaved(budget: EnvironmentBudgetDto): void;
}) {
  useI18n();
  const [initial] = useState(() => ({ maxRunning: String(budget.maxRunning), maxCpu: String(budget.maxCpu),
    maxMemory: String(budget.maxMemoryBytes / GIB), reservedCpu: String(budget.reservedCpu), reservedMemory: String(budget.reservedMemoryBytes / GIB) }));
  const [draft, setDraft] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const formId = useId();
  const fields = ['maxRunning', 'maxCpu', 'maxMemory', 'reservedCpu', 'reservedMemory'] as const;
  const valid = fields.every(field => positive(draft[field], field === 'maxRunning', field.startsWith('reserved')))
    && Number(draft.reservedCpu) <= Number(draft.maxCpu) && Number(draft.reservedMemory) <= Number(draft.maxMemory)
    && Number.isSafeInteger(Number(draft.maxMemory) * GIB) && Number.isSafeInteger(Number(draft.reservedMemory) * GIB);
  const dirty = fields.some(field => initial[field] !== draft[field]);
  const save = async () => {
    if (!valid || busy) return;
    setBusy(true); setError(null);
    try { onSaved(await saveEnvironmentBudget({ maxRunning: Number(draft.maxRunning), maxCpu: Number(draft.maxCpu), maxMemoryBytes: Number(draft.maxMemory) * GIB,
      reservedCpu: Number(draft.reservedCpu), reservedMemoryBytes: Number(draft.reservedMemory) * GIB }, deviceId)); }
    catch (cause) { setError(environmentError(cause)); }
    finally { setBusy(false); }
  };
  return <EnvironmentWindow title={t('editBudget')} dirty={dirty} busy={busy} onClose={onClose}
    footer={close => <div className="env-form-actions">
      {!valid && <p className="env-validation">{t('numericInvalid')}</p>}
      {error && <ErrorMark message={error} onRetry={() => void save()} />}
      <button type="button" className="relay-button-secondary" onClick={close} disabled={busy}>{t('cancel')}</button>
      <button type="submit" form={formId} className="relay-button-primary" disabled={!valid || busy}>{t(busy ? 'saving' : 'save')}</button>
    </div>}>
    <form id={formId} className="env-form" onSubmit={event => { event.preventDefault(); void save(); }}>
      <p className="env-note">{t('budgetHint')}</p>
      <fieldset disabled={busy} className="env-fieldset is-budget">{fields.map(field => <label key={field} className={field === 'maxRunning' ? 'is-wide' : undefined}><span>{t(field as EnvironmentMessage)}</span>
        <input className="relay-input" type="number" min={field.startsWith('reserved') ? 0 : field === 'maxRunning' ? 1 : 0.01} step={field === 'maxRunning' ? 1 : 'any'}
          value={draft[field]} onChange={event => { setDraft(current => ({ ...current, [field]: event.target.value })); setError(null); }} />
      </label>)}</fieldset>
    </form>
  </EnvironmentWindow>;
}
