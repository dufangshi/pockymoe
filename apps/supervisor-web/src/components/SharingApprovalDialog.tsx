import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useState, useSyncExternalStore } from 'react';
import {
  currentGrantReview,
  finishGrantReview,
  subscribeGrantReview,
} from '../lib/sharingApproval';
import { FormDialog } from './FormDialog';

export function SharingApprovalDialog() {
  useI18n();
  const review = useSyncExternalStore(subscribeGrantReview, currentGrantReview);
  return review ? <GrantReviewDialog key={review.id} review={review} /> : null;
}
function GrantReviewDialog({ review }: {
  review: NonNullable<ReturnType<typeof currentGrantReview>>;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const cancel = () => finishGrantReview(review.id, false);
  return (
    <FormDialog title={translate('sharing.reviewGrantTitle')} onClose={cancel}>
      <p className="whitespace-pre-wrap break-words text-sm">
        {review.description}
      </p>
      {!review.recipientVerified && (
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={acknowledged}
            onChange={(event) => setAcknowledged(event.target.checked)}
          />
          <span>{translate('sharing.acknowledgeUnverified')}</span>
        </label>
      )}
      <div className="flex justify-end gap-2">
        <button className="relay-button-secondary px-3 py-2" onClick={cancel}>
          {translate('sharing.cancel')}
        </button>
        <button
          className="relay-button-primary px-3 py-2"
          disabled={!review.recipientVerified && !acknowledged}
          onClick={() => finishGrantReview(review.id, true)}
        >
          {translate('sharing.signReviewed')}
        </button>
      </div>
    </FormDialog>
  );
}
