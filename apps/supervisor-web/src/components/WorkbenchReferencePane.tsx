import { useMemo } from 'react';
import { GraphChatThreadChatPanel, ThreadLoadingIndicator, createWorkspacePathResolver, type ThreadComposerProps } from '@pockymoe/thread-ui';
import { translate as t, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  buildThreadImageAssetUrl,
  canReadLinkedFiles,
  fetchThreadHistoryItemDetail,
  fetchThreadTurnDetail,
  fetchWorkspaceFileTree,
  fetchLinkedFile,
} from '../lib/api';
import type { useWorkbenchReference } from '../pages/useWorkbenchReference';
import { resolveThreadContextUsage } from '../pages/threadDetailModel';

export function WorkbenchReferencePane({
  controller,
  onOpenThread,
  draft, onDraftChange, sendShortcut, onOpenWorkspaceFile, onOpenHarness,
}: {
  controller: ReturnType<typeof useWorkbenchReference>;
  onOpenThread: (id: string) => void;
  draft: { prompt: string; attachments: NonNullable<ThreadComposerProps['draftAttachments']> };
  onDraftChange: ThreadComposerProps['onDraftChange'];
  sendShortcut: ThreadComposerProps['sendShortcut'];
  onOpenHarness?: (composerFocusOwner?: string) => void;
  onOpenWorkspaceFile?: (input: {path: string; line?: number}) => void;
}) {
  useI18n();
  const { detail, error, loadEarlier, loadingEarlier, retry } = controller;
  const threadId = detail?.thread.id;
  const adapter = useMemo(
    () => ({
      getImageAssetUrl: ({
        threadId,
        path,
        variant,
      }: {
        threadId: string;
        path: string;
        variant?: 'thumbnail';
      }) => buildThreadImageAssetUrl(threadId, { path, variant }, controller.deviceId),
      onLoadHistoryItemDetail: (itemId: string) =>
        fetchThreadHistoryItemDetail(threadId!, itemId, controller.deviceId),
      onLoadTurnDetail: (turnId: string) =>
        fetchThreadTurnDetail(threadId!, turnId, controller.deviceId),
      onOpenLinkedThread: onOpenThread,
      ...(onOpenWorkspaceFile ? {onOpenWorkspaceFile} : {}),
      workspaceRootPath: detail?.workspace.absPath,
      ...(detail ? { resolveWorkspacePath: createWorkspacePathResolver({
        listTree: input => fetchWorkspaceFileTree(detail.workspace.id, { path: input.path ?? null }, controller.deviceId),
        ...(controller.deviceId === 'local' || canReadLinkedFiles(controller.access) ? { statLinkedFile: (input: {threadId: string; path: string}) => fetchLinkedFile(input.threadId, input.path, controller.deviceId) } : {}),
      }, { threadId: detail.thread.id, workspaceId: detail.workspace.id }, detail.workspace.absPath) } : {}),
    }),
    [threadId, onOpenThread, controller.deviceId, onOpenWorkspaceFile, detail?.workspace.absPath, detail?.workspace.id, controller.access],
  );
  return (
    <>
      {error && (
        <div className="workbench-reference-status" role="status">
          {!detail && <p>{t('workbench.referenceUnavailable')}</p>}
          <p>{error}</p>
          <button onClick={retry}>{t('workbench.retryReference')}</button>
        </div>
      )}
      {detail ? (
        <>
          <GraphChatThreadChatPanel
            key={detail.thread.id}
            detail={detail}
            floatingDesktopComposer
            adapter={{
              openThread: onOpenThread,
              sendPrompt: controller.send,
              ...(controller.canControl ? {
                cancelPendingSteer: async (_threadId, queueId) => { await controller.cancelQueued(queueId); },
                ...(controller.capabilities?.turns.steer ? { steerPendingPrompt: async (_threadId: string, queueId: string) => { await controller.steerQueued(queueId); } } : {}),
              } : {}),
            }}
            timelineAdapter={adapter}
            transcriptItemCount={detail.turns.reduce((total, turn) => total + turn.items.length, 0)}
            timelineProps={{
              liveItems: detail.liveItems ?? null,
              respondingRequestId: controller.respondingRequestId,
              ...(controller.canControl ? { onRespondToRequest: controller.respond } : {}),
              answeredRequestNotes: detail.answeredRequestNotes ?? [],
              activityNotes: detail.activityNotes ?? [],
              onLoadEarlier: loadEarlier,
              loadingEarlier,
            }}
            composerProps={{
              sendShortcut: sendShortcut ?? 'ctrlEnter',
              busy: controller.busy,
              settingsBusy: controller.busy,
              error: null,
              shellAvailable: false,
              model: detail.thread.model,
              agentLabel: detail.thread.agentId ?? detail.thread.provider,
              reasoningEffort: detail.thread.reasoningEffort,
              collaborationMode: detail.thread.collaborationMode,
              sandboxMode: detail.thread.sandboxMode ?? null,
              fastMode: detail.thread.fastMode ?? false,
              modelOptions: controller.models,
              contextUsage: resolveThreadContextUsage(detail),
              capabilities: controller.capabilities ? {
                ...controller.capabilities,
                turns: { ...controller.capabilities.turns, compact: false },
                branching: { ...controller.capabilities.branching, fork: false, resumeAt: false, forkAt: false },
                controls: { ...controller.capabilities.controls, goals: false },
                management: { ...controller.capabilities.management, skills: false, mcpStatus: false, hooks: false, hostConfigFiles: false },
              } : null,
              threadConnected: detail.thread.isLoaded,
              disabled: Boolean(controller.disabledReason),
              disabledPlaceholder: controller.disabledReason ?? undefined,
              draftPrompt: draft.prompt,
              draftAttachments: draft.attachments,
              onDraftChange,
              canInterrupt: Boolean(controller.canControl && detail.thread.activeTurnId),
              ...(controller.canControl ? {
                ...(onOpenHarness ? { onOpenHarness } : {}),
                onInterrupt: async () => { await controller.interrupt(); },
                onUpdateSettings: async input => { await controller.updateSettings(input); },
              } : {}),
            }}
          />
        </>
      ) : (
        !error && (
          <ThreadLoadingIndicator />
        )
      )}
    </>
  );
}
