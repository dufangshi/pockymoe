import { useState } from 'react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { saveEnvironmentConnection, type EnvironmentConnectionDto } from '../../lib/environmentApi';
import { environmentText as t, type EnvironmentMessage } from '../../lib/environmentI18n';
import { EnvironmentWindow } from './EnvironmentWindow';
import { GIB, environmentError, positive } from './model';

export function ConnectionEditor({ connection, deviceId, onClose, onSaved }: {
  connection: EnvironmentConnectionDto | null; deviceId: string | null;
  onClose(): void; onSaved(connection: EnvironmentConnectionDto): void;
}) {
  useI18n();
  const [initial] = useState(() => ({
    name: connection?.name ?? '', baseUrl: connection?.baseUrl ?? '', apiKey: '',
    dokployEnvironmentId: connection?.dokployEnvironmentId ?? '', image: connection?.image ?? '',
    dockerHost: connection?.dockerHost ?? '', allowedRoots: connection?.allowedRoots.join('\n') ?? '',
    allowedEgressHosts: connection?.allowedEgressHosts?.join('\n') ?? '',
    cpu: String(connection?.cpuLimit ?? 1), memory: String((connection?.memoryLimitBytes ?? GIB) / GIB),
    pids: String(connection?.pidsLimit ?? 512), nofile: String(connection?.nofileLimit ?? 1024),
  }));
  const [draft, setDraft] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempted, setAttempted] = useState(false);
  const dirty = Object.keys(initial).some(key => draft[key as keyof typeof draft] !== initial[key as keyof typeof initial]);
  const change = (field: keyof typeof draft, value: string) => { setDraft(current => ({ ...current, [field]: value })); setError(null); };
  const roots = draft.allowedRoots.split('\n').map(value => value.trim()).filter(Boolean);
  const egressHosts = draft.allowedEgressHosts.split('\n').map(value => value.trim().toLowerCase()).filter(Boolean);
  const validEgress = egressHosts.every(host => host.length <= 253 && host.includes('.') && /[a-z]/.test(host) && host.split('.').every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label)));
  let validUrl = false;
  try { const url = new URL(draft.baseUrl); validUrl = ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash; } catch { /* Field validation below. */ }
  const valid = !!draft.name.trim() && validUrl && !!draft.dokployEnvironmentId.trim() && !!draft.image.trim()
    && (!!connection?.configured || !!draft.apiKey) && roots.length > 0 && roots.every(root => root.startsWith('/') && !root.includes('\0'))
    && positive(draft.cpu) && positive(draft.memory) && positive(draft.pids, true) && positive(draft.nofile, true)
    && Number.isSafeInteger(Number(draft.memory) * GIB) && validEgress;
  const save = async () => {
    setAttempted(true);
    if (!valid || busy) return;
    setBusy(true); setError(null);
    try {
      const next = await saveEnvironmentConnection(connection?.id ?? 'new', {
        name: draft.name.trim(), baseUrl: draft.baseUrl.trim(), ...(draft.apiKey ? { apiKey: draft.apiKey } : {}),
        dokployEnvironmentId: draft.dokployEnvironmentId.trim(), image: draft.image.trim(), storageNode: 'local',
        ...(draft.dockerHost.trim() ? { dockerHost: draft.dockerHost.trim() } : {}), allowedRoots: roots,
        allowedEgressHosts: [...new Set(egressHosts)],
        cpuLimit: Number(draft.cpu), memoryLimitBytes: Number(draft.memory) * GIB, pidsLimit: Number(draft.pids), nofileLimit: Number(draft.nofile),
      }, deviceId);
      setDraft(current => ({ ...current, apiKey: '' }));
      onSaved(next);
    } catch (cause) { setError(environmentError(cause)); }
    finally { setBusy(false); }
  };
  const textField = (field: keyof typeof draft, label: EnvironmentMessage, type = 'text', hint?: EnvironmentMessage) => <label>
    <span>{t(label)}</span><input className="relay-input" type={type} value={draft[field]} disabled={busy}
      autoComplete={field === 'apiKey' ? 'new-password' : 'off'} spellCheck={false} onChange={event => change(field, event.target.value)} />
    {hint && <span className="environment-hint">{t(hint)}</span>}
  </label>;
  return <EnvironmentWindow title={t(connection ? 'editConnection' : 'addConnection')} dirty={dirty} busy={busy} onClose={onClose}>
    {close => <form className="environment-form" noValidate onSubmit={event => { event.preventDefault(); void save(); }}>
      <fieldset disabled={busy}>
        {textField('name', 'name')}{textField('baseUrl', 'baseUrl', 'url')}
        {textField('apiKey', 'apiKey', 'password', 'keyHint')}
        {textField('dokployEnvironmentId', 'dokployEnvironmentId')}{textField('image', 'image', 'text', 'imageHint')}
        {textField('dockerHost', 'dockerHost')}
        <p className="environment-hint">{t('localStorage')}</p>
        <label><span>{t('allowedRoots')}</span><textarea className="relay-input" spellCheck={false} value={draft.allowedRoots} onChange={event => change('allowedRoots', event.target.value)} /><span className="environment-hint">{t('rootsHint')}</span></label>
        <label><span>{t('allowedEgressHosts')}</span><textarea className="relay-input" spellCheck={false} value={draft.allowedEgressHosts} aria-invalid={!validEgress || undefined} onChange={event => change('allowedEgressHosts', event.target.value)} /><span className="environment-hint">{t('egressHint')}</span></label>
        {!validEgress && <p className="environment-validation">{t('egressInvalid')}</p>}
      </fieldset>
      <fieldset disabled={busy}><legend>{t('defaultLimits')}</legend><div className="environment-form-grid">
        {(['cpu', 'memory', 'pids', 'nofile'] as const).map(field => <label key={field}><span>{t(field)}</span>
          <input type="number" className="relay-input" min={field === 'cpu' || field === 'memory' ? '0.01' : '1'} step={field === 'cpu' || field === 'memory' ? 'any' : '1'} value={draft[field]}
            onChange={event => change(field, event.target.value)} aria-invalid={!positive(draft[field], field === 'pids' || field === 'nofile') || undefined} />
        </label>)}
      </div></fieldset>
      {attempted && !valid && <p className="environment-validation" role="status">{t('invalidConnection')}</p>}
      <footer className="environment-actions"><button type="button" className="host-secondary-button" onClick={close} disabled={busy}>{t('cancel')}</button>
        <button type="submit" className="relay-button-primary" disabled={busy}>{t(busy ? 'saving' : 'save')}</button>
        {error && <ErrorMark message={error} onRetry={() => void save()} />}
      </footer>
    </form>}
  </EnvironmentWindow>;
}
