import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useState, useSyncExternalStore } from 'react';
import {
  currentGrantReview,
  finishGrantReview,
  subscribeGrantReview,
  type GrantChange,
  type GrantSummary,
} from '../lib/sharingApproval';
import { FormDialog } from './FormDialog';

export function SharingApprovalDialog() {
  useI18n();
  const review = useSyncExternalStore(subscribeGrantReview, currentGrantReview);
  return review ? <GrantReviewDialog key={review.id} review={review} /> : null;
}

const scopeText = (grant: GrantSummary) => {
  if (grant.scope === 'thread') return translate('sharing.scopeThread');
  if (grant.scope === 'workspace') return translate('sharing.scopeWorkspace');
  return grant.workspaceScope === 'all'
    ? translate('sharing.scopeAllWorkspaces')
    : translate('sharing.scopeSelectedWorkspaces', { count: grant.workspaceCount });
};
const threadText = (grant: GrantSummary) => translate(grant.threadAccess === 'control' ? 'sharing.threadControl' : 'sharing.threadRead');
const filesText = (grant: GrantSummary) => grant.workspaceAccess === 'write'
  ? translate('sharing.readAndEdit')
  : grant.workspaceAccess === 'read' ? translate('sharing.readFiles') : translate('sharing.filesNone');
const newThreadsText = (grant: GrantSummary) => translate(grant.canCreateThreads ? 'sharing.allowed' : 'sharing.notAllowed');
const expiryText = (grant: GrantSummary) => grant.expiresAt === null
  ? translate('sharing.noExpiry')
  : new Date(grant.expiresAt).toLocaleString();
const changeText: Record<GrantChange['field'], [Parameters<typeof translate>[0], (grant: GrantSummary) => string]> = {
  scope: ['sharing.reviewScope', scopeText],
  threadAccess: ['sharing.reviewThreads', threadText],
  workspaceAccess: ['sharing.reviewFiles', filesText],
  canCreateThreads: ['sharing.reviewNewThreads', newThreadsText],
  expiresAt: ['sharing.reviewExpiry', expiryText],
  recipient: ['sharing.reviewRecipient', grant => grant.recipient.name],
};

/** Review of a device grant before the account key signs it: what it allows,
    in words, then the exact signed content folded away. Nothing scrolls
    sideways; every value wraps. */
function GrantReviewDialog({ review }: {
  review: NonNullable<ReturnType<typeof currentGrantReview>>;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const cancel = () => finishGrantReview(review.id, false);
  const grant = review.summary;
  return (
    <FormDialog title={translate('sharing.reviewGrantTitle')} onClose={cancel} className="grant-review">
      {grant ? <>
        <p className="grant-review-lead">{translate('sharing.reviewLead', { recipient: grant.recipient.name, device: grant.device.name })}</p>
        <dl className="grant-review-rows">
          <div><dt>{translate('sharing.reviewRecipient')}</dt><dd>
            {grant.recipient.name}
            <span className={`grant-review-badge ${review.recipientVerified ? 'is-verified' : 'is-unverified'}`}>
              {translate(review.recipientVerified ? 'sharing.reviewVerified' : 'sharing.reviewUnverified')}
            </span>
          </dd></div>
          <div><dt>{translate('sharing.reviewDevice')}</dt><dd>{grant.device.name}</dd></div>
          <div><dt>{translate('sharing.reviewScope')}</dt><dd>{scopeText(grant)}</dd></div>
          <div><dt>{translate('sharing.reviewThreads')}</dt><dd>{threadText(grant)}</dd></div>
          <div><dt>{translate('sharing.reviewFiles')}</dt><dd>{filesText(grant)}</dd></div>
          <div><dt>{translate('sharing.reviewNewThreads')}</dt><dd>{newThreadsText(grant)}</dd></div>
          <div><dt>{translate('sharing.reviewExpiry')}</dt><dd>{expiryText(grant)}</dd></div>
          <div><dt>{translate('sharing.reviewSafetyCode')}</dt><dd>
            <code>{grant.safetyCode}</code>
            <small>{translate('sharing.reviewSafetyCodeHint')}</small>
          </dd></div>
        </dl>
        {grant.changes && grant.changes.length > 0 && (
          <section className="grant-review-changes">
            <h3>{translate('sharing.reviewChangesTitle')}</h3>
            <ul>
              {grant.changes.map(change => {
                const [label, format] = changeText[change.field];
                return <li key={change.field}><b>{translate(label)}</b><span>{format(change.before)} → {format(change.after)}</span></li>;
              })}
            </ul>
          </section>
        )}
        <details className="grant-review-technical">
          <summary>{translate('sharing.reviewTechnical')}</summary>
          <pre>{review.description}</pre>
        </details>
      </> : <pre className="grant-review-technical-text">{review.description}</pre>}
      {!review.recipientVerified && (
        <label className="grant-review-acknowledge">
          <input
            type="checkbox"
            checked={acknowledged}
            onChange={(event) => setAcknowledged(event.target.checked)}
          />
          <span>{translate('sharing.acknowledgeUnverified')}</span>
        </label>
      )}
      <p className="grant-review-note">{translate('sharing.signReviewed')}</p>
      <div className="grant-review-actions">
        <button className="relay-button-secondary px-3 py-2" onClick={cancel}>
          {translate('sharing.cancel')}
        </button>
        <button
          className="relay-button-primary px-3 py-2"
          disabled={!review.recipientVerified && !acknowledged}
          onClick={() => finishGrantReview(review.id, true)}
        >
          {translate('sharing.signAndShare')}
        </button>
      </div>
    </FormDialog>
  );
}
