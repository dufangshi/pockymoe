import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useNavigate, useSearchParams } from 'react-router-dom';

import { FloatingRoutePanel } from '../components/FloatingRoutePanel';
import {
  currentThreadHref,
  currentThreadsHref,
  currentWorkspacesHref,
} from '../lib/relayRoutes';
import { ThreadCreateForm } from './thread-create/ThreadCreateForm';

export function ThreadNewPage() {
  useI18n();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const requestedWorkspaceId = searchParams.get('workspaceId');
  const requestedTitle = searchParams.get('title');

  function handleCancel() {
    if (requestedWorkspaceId) {
      navigate(currentThreadsHref(requestedWorkspaceId));
      return;
    }

    navigate(currentWorkspacesHref());
  }

  return (
    <FloatingRoutePanel
      backLabel={requestedWorkspaceId ? translate("workbench.backToThreads") : translate("workbench.backToWorkspaces")}
      // One name, no repeated heading or explanation (§13, compact screens).
      title={translate("workbench.newThread")}
      maxWidthClassName="!max-w-3xl"
      onBack={handleCancel}
    >
      <ThreadCreateForm
        initialWorkspaceId={requestedWorkspaceId}
        initialTitle={requestedTitle}
        onCancel={handleCancel}
        onCreated={(thread) => navigate(currentThreadHref(thread.id))}
      />
    </FloatingRoutePanel>
  );
}
