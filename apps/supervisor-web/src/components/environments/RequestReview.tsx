import { useState } from 'react';
import { ArrowRight, ChevronRight } from 'lucide-react';
import { CopyButton, ErrorMark, PathChip, StatusMark, formatShortTimestamp } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { decideEnvironmentRequest, type EnvironmentRequestDto } from '../../lib/environmentApi';
import { environmentText as et } from '../../lib/environmentI18n';
import { environmentRequestText as t } from '../../lib/environmentRequestI18n';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';
import { EnvironmentWindow } from './EnvironmentWindow';
import { Fact, MigrationSteps, Tag, limitsText, requestMark } from './ui';

type Spec = EnvironmentRequestDto['spec'];
type Mount = Spec['credentialMounts'][number];
type Decision = 'approve' | 'reject' | 'retry' | 'recoverHost';

const purposeLabel = (purpose: Mount['purpose']) =>
  purpose === 'github' ? 'GitHub CLI' : purpose === 'codex' ? 'Codex' : purpose === 'claude' ? 'Claude' : t('purposeGeneric');

/** The one owner review of an agent's isolation proposal, used by the thread
    window and Settings. Every granted path, mode and host stays visible; only
    identifiers and the long migration contract fold away (UI §13, §14). */
export function RequestReview({ request, deviceId, canManage, onUpdate, onClose }: {
  request: EnvironmentRequestDto; deviceId?: string | null | undefined; canManage: boolean;
  onUpdate(request: EnvironmentRequestDto): void; onClose(): void;
}) {
  useI18n();
  const [busy, setBusy] = useState<Decision | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recover, setRecover] = useState(false);
  const spec = request.spec;
  const act = async (action: Decision) => {
    if (busy || !canManage) return;
    setBusy(action); setError(null);
    try { onUpdate(await decideEnvironmentRequest(request, action, deviceId)); setRecover(false); }
    catch { setError(t('decisionFailed')); }
    finally { setBusy(null); }
  };
  const decisions = request.state === 'pending' || request.state === 'failed' || request.state === 'ready';
  const hint = request.state === 'waitingIdle' ? t('waitingHint') : request.state === 'provisioning' ? t('provisioningHint')
    : request.state === 'failed' ? t('fencedHint') : request.state === 'ready' ? t('readyHint') : null;
  return <EnvironmentWindow wide title={spec.name} busy={!!busy} onClose={onClose}
    subtitle={<span className="env-request-status"><StatusMark status={requestMark(request.state)} label={t(request.state)} />
      <span>{t(request.state)}</span><span className="env-dim">· {t('submitted', { time: formatShortTimestamp(request.createdAt) })}</span></span>}
    footer={decisions ? <div className="env-decision">
      {request.state === 'pending' && <p className="env-decision-note">{t('ownerShort')}</p>}
      <div className="env-decision-actions">
        {error && <ErrorMark message={error} detail={t('owner')} />}
        {!canManage && <ErrorMark kind="unavailable" message={t('readOnlyReason')} />}
        {request.state === 'pending' && <>
          <button type="button" className="relay-button-secondary" disabled={!!busy || !canManage} onClick={() => void act('reject')}>{busy === 'reject' ? t('working') : t('reject')}</button>
          <button type="button" className="relay-button-primary" disabled={!!busy || !canManage} onClick={() => void act('approve')}>{busy === 'approve' ? t('working') : t('approve')}</button>
        </>}
        {(request.state === 'failed' || request.state === 'ready') && <button type="button" className="relay-button-secondary env-danger-button" disabled={!!busy || !canManage} onClick={() => setRecover(true)}>{t('recoverHost')}</button>}
        {request.state === 'failed' && <button type="button" className="relay-button-primary" disabled={!!busy || !canManage} onClick={() => void act('retry')}>{busy === 'retry' ? t('working') : t('retry')}</button>}
      </div>
    </div> : undefined}>
    <div className="env-review" data-request-id={request.id}>
      <MigrationSteps state={request.state} />
      {hint && <p className="env-review-hint" data-state={request.state}>{hint}
        {request.lastError && <ErrorMark message={t('failedReason')} detail={request.lastError} announce={false} />}</p>}
      <p className="env-lead">{t('lead', { connection: spec.connection.name })}</p>
      <dl className="env-facts">
        <Fact label={t('runsOn')} note={<code>{spec.connection.image}</code>}>{spec.connection.name} <span className="env-dim">· Dokploy</span></Fact>
        <Fact label={t('project')}><PathChip path={spec.workspacePath} label={et('copyAbsolutePath')} /></Fact>
        <Fact label={t('workingCopy')} note={spec.workingPath === spec.workspacePath ? t('sameAsProject') : undefined}><PathChip path={spec.workingPath} label={et('copyAbsolutePath')} /></Fact>
        <Fact label={t('resources')} note={et('diskQuota')}>{limitsText(spec.limits)}</Fact>
        <Fact label={t('model')}>{spec.harness} · {spec.model ?? t('modelDefault')} · {spec.reasoningEffort ?? t('modelDefault')}</Fact>
        <Fact label={t('authentication')}><div className="env-auth">
          {(['codex', 'claude'] as const).map(harness => {
            const native = spec.authentication[harness] === 'nativeFile';
            const unused = (spec.harness === 'codex' || spec.harness === 'claude') && spec.harness !== harness;
            return <div key={harness} className="env-auth-row" data-unused={unused || undefined}>
              <strong>{harness === 'codex' ? 'Codex' : 'Claude'}</strong>
              <Tag tone={native ? 'warning' : 'success'}>{t(native ? 'native' : 'broker')}</Tag>
              {unused && <span className="env-dim">{t('otherHarness')}</span>}
              <small className="env-note">{t(native ? 'nativeNote' : 'brokerNote')}</small>
            </div>;
          })}
        </div></Fact>
        <Fact label={<>{t('credentials')}{spec.credentialMounts.length > 0 && <span className="env-heading-count">{spec.credentialMounts.length}</span>}</>}>
          {!spec.credentialMounts.length ? <span className="env-dim">{t('none')}</span> : <>
            <ul className="env-grants">{spec.credentialMounts.map((mount, index) => <Grant key={mount.target} mount={mount} kind={spec.credentialMountKinds[index] ?? 'file'} />)}</ul>
            <small className="env-note">{t('direct')} {t('stores')}</small>
          </>}
        </Fact>
        <Fact label={t('network')} note={spec.egressHosts.length ? t('networkHint') : undefined}>
          {spec.egressHosts.length ? <ul className="env-hosts">{spec.egressHosts.map(host => <li key={host}><code>{host}</code></li>)}</ul> : <span className="env-dim">{t('offline')}</span>}
        </Fact>
        <Fact label={t('migration')}>{t('migrationShort')}
          <details className="env-more"><summary><ChevronRight aria-hidden="true" />{t('details')}</summary><p>{t('migrationHint')}</p></details>
        </Fact>
        {spec.handoff && <Fact label={t('handoff')}><Handoff text={spec.handoff} /></Fact>}
      </dl>
      <details className="env-technical">
        <summary><ChevronRight aria-hidden="true" />{t('technical')}</summary>
        <dl className="env-facts is-compact">
          <Fact label={t('digest')}><span className="env-id"><code>{request.digest}</code><CopyButton value={request.digest} label={t('copyDigest')} /></span></Fact>
          <Fact label={t('requestId')}><span className="env-id"><code>{request.id}</code><CopyButton value={request.id} label={t('copyRequest')} /></span></Fact>
          <Fact label={t('identity')}><span className="env-id"><code>{request.threadId}</code><CopyButton value={request.threadId} label={t('copyThread')} /></span></Fact>
        </dl>
        <p className="env-note">{t('owner')}</p>
      </details>
    </div>
    <EnvironmentConfirmation open={recover} title={t('recoveryTitle')} description={t('recoveryBody')} confirmLabel={t('recoveryConfirm')} busy={!!busy} busyLabel={t('working')} error={error}
      onCancel={() => { if (!busy) setRecover(false); }} onConfirm={() => void act('recoverHost')} />
  </EnvironmentWindow>;
}

function Grant({ mount, kind }: { mount: Mount; kind: 'directory' | 'file' }) {
  const native = mount.purpose === 'codex' || mount.purpose === 'claude';
  const note = mount.readOnly ? (kind === 'file' ? 'readOnlyHint' : 'readOnlyDirectory') : native ? 'refresh' : 'writableHint';
  return <li className="env-grant">
    <div className="env-grant-tags">
      <Tag tone="accent">{purposeLabel(mount.purpose)}</Tag>
      <Tag>{t(kind === 'directory' ? 'directory' : 'file')}</Tag>
      <Tag tone={mount.readOnly ? 'neutral' : 'warning'}>{t(mount.readOnly ? 'readOnly' : 'readWrite')}</Tag>
    </div>
    <div className="env-grant-paths">
      <span className="env-grant-path"><span className="sr-only">{t('source')}</span><PathChip path={mount.source} label={et('copyAbsolutePath')} /></span>
      <ArrowRight aria-hidden="true" className="env-grant-arrow" />
      <span className="env-grant-path"><span className="sr-only">{t('target')}</span><PathChip path={mount.target} label={et('copyAbsolutePath')} /></span>
    </div>
    {Object.entries(mount.environment).map(([key, path]) => <div key={key} className="env-grant-var"><code>{key}</code><span aria-hidden="true">=</span><PathChip path={path} label={et('copyAbsolutePath')} /></div>)}
    <small className="env-note">{t(note)}</small>
  </li>;
}

/** The agent's handoff is untrusted text: quoted, wrapped, and folded when long. */
function Handoff({ text }: { text: string }) {
  const [whole, setWhole] = useState(false);
  const long = text.length > 320 || text.split('\n').length > 6;
  return <div className="env-handoff">
    <blockquote data-folded={long && !whole ? true : undefined}>{text}</blockquote>
    <small className="env-note">{t('handoffNote')}</small>
    {long && <button type="button" className="env-link" aria-expanded={whole} onClick={() => setWhole(value => !value)}>{t(whole ? 'showLess' : 'showAll')}</button>}
  </div>;
}
