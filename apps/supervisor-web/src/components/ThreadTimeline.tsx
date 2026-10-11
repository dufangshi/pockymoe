import {
  ThreadTimeline as SharedThreadTimeline,
  type ThreadTimelineAdapter,
} from '@pockymoe/thread-ui';
import type { ComponentProps } from 'react';
import { buildThreadImageAssetUrl } from '../lib/api';
import { currentThreadHref } from '../lib/relayRoutes';

type ThreadTimelineProps = ComponentProps<typeof SharedThreadTimeline> & {
  onLoadHistoryItemDetail?: ThreadTimelineAdapter['onLoadHistoryItemDetail'];
  onOpenThread?: (threadId: string) => void;
};

export function ThreadTimeline({
  adapter,
  onLoadHistoryItemDetail,
  onOpenThread,
  ...props
}: ThreadTimelineProps) {
  const localAdapter: ThreadTimelineAdapter = {
    getImageAssetUrl: ({ threadId, path, variant }) =>
      buildThreadImageAssetUrl(threadId, { path, variant }),
    onOpenLinkedThread:
      onOpenThread ??
      ((threadId) => {
        window.location.assign(currentThreadHref(threadId));
      }),
    ...(onLoadHistoryItemDetail
      ? { onLoadHistoryItemDetail }
      : {}),
    ...adapter,
  };

  return <SharedThreadTimeline {...props} adapter={localAdapter} />;
}
