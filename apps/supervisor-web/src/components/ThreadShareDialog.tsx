import { translate } from '@pockymoe/thread-ui/i18n';
import {
  ThreadActionsDialog,
  type CreateThreadShareInput,
  type ThreadShareSummary,
} from '@pockymoe/thread-ui';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ApiError,
  createRelayGrant,
  createRelayShare,
  fetchRelayPortal,
  revokeRelayGrant,
  revokeRelayShare,
  updateRelayGrant,
  updateRelayShare,
} from '../lib/api';
import { sharingEditInput, type SharingInput } from '../lib/signedSharing';
import { SignedShareDetails } from './SignedShareDetails';

/** The thread a share window is for, on a device the account owns. */
export interface ThreadShareTarget {
  deviceId: string;
  threadId: string;
  threadTitle: string;
  workspaceId: string | null;
  workspaceLabel: string | null;
}

export interface ThreadShareState {
  status: 'idle' | 'loading' | 'ready' | 'failed';
  shares: ThreadShareSummary[];
  error: string | null;
}

type MessageKey = Parameters<typeof translate>[0];

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error && caught.message
      ? caught.message
      : fallback;
}

/** Who a thread is shared with and the changes to that: this thread, its
    workspace or the whole device (docs/account-key-trust.md#sharing). The thread
    page's share window and a thread's menu use the same one. `onError` hears a
    failed create or revoke, which the thread page also shows above the thread. */
export function useThreadShares(target: ThreadShareTarget | null, onError?: (message: string) => void) {
  const [state, setState] = useState<ThreadShareState>({ status: 'idle', shares: [], error: null });
  const [busy, setBusy] = useState(false);
  const contexts = useRef(new Map<string, SharingInput>());
  // Read when called, so a thread renamed meanwhile is shared by its new title.
  const targetRef = useRef(target);
  targetRef.current = target;
  const errorRef = useRef(onError);
  errorRef.current = onError;

  const load = useCallback(async () => {
    const current = targetRef.current;
    if (!current) {
      setState({ status: 'ready', shares: [], error: null });
      return;
    }
    setState(previous => ({ ...previous, status: 'loading', error: null }));
    try {
      const portal = await fetchRelayPortal();
      const ownedShares = portal.sharedByMe.filter(share =>
        share.deviceId === current.deviceId && share.threadId === current.threadId);
      // A share keeps the thread's and workspace's current names.
      await Promise.all(ownedShares
        .filter(share => share.threadTitle !== current.threadTitle
          || (share.workspaceId && share.workspaceLabel !== current.workspaceLabel))
        .map(share => updateRelayShare(share.id, {
          threadTitle: current.threadTitle,
          ...(share.workspaceId ? { workspaceLabel: current.workspaceLabel } : {}),
        })));
      // The whole device, and this thread's workspace, are shared from here too.
      const ownedGrants = (portal.grantsByMe ?? []).filter(grant => grant.deviceId === current.deviceId && (
        grant.scope === 'device'
        || (grant.scope === 'workspace' && Boolean(current.workspaceId) && grant.workspaceId === current.workspaceId)));
      contexts.current = new Map([
        ...ownedShares.map(share => [share.id, sharingEditInput('share', share)] as const),
        ...ownedGrants.map(grant => [`grant:${grant.id}`, sharingEditInput('grant', grant)] as const),
      ]);
      const shares: ThreadShareSummary[] = ownedShares.map(share => ({
        id: share.id,
        trustContent: <SignedShareDetails kind="share" row={share} />,
        targetUsername: share.targetUsername,
        label: share.label,
        threadAccess: share.threadAccess,
        workspaceAccess: share.workspaceAccess,
        createdAt: share.createdAt,
      }));
      shares.push(...ownedGrants.map(grant => ({
        trustContent: <SignedShareDetails kind="grant" row={grant} />,
        id: `grant:${grant.id}`, scope: grant.scope === 'workspace' ? 'workspace' as const : 'device' as const,
        targetUsername: grant.targetUsername, label: grant.label,
        threadAccess: grant.threadAccess, workspaceAccess: grant.workspaceAccess, canCreateThreads: grant.canCreateThreads,
        workspaceLabel: grant.workspaceLabel ?? current.workspaceLabel, createdAt: grant.createdAt,
      })));
      setState({ status: 'ready', shares, error: null });
    } catch (caught) {
      setState(previous => ({ ...previous, status: 'failed', error: errorMessage(caught, translate('workbench.unableToLoadActiveShares')) }));
    }
  }, []);

  const fail = useCallback((message: string) => {
    setState(previous => ({ ...previous, status: 'failed', error: message }));
  }, []);

  // A change reloads the list; its failure shows in the window and is rethrown,
  // so the form keeps what was typed.
  const run = useCallback(async (action: () => Promise<unknown>, fallback: MessageKey, report: boolean) => {
    setBusy(true);
    setState(previous => ({ ...previous, error: null }));
    try {
      await action();
      await load();
    } catch (caught) {
      const message = errorMessage(caught, translate(fallback));
      fail(message);
      if (report) errorRef.current?.(message);
      throw caught;
    } finally {
      setBusy(false);
    }
  }, [fail, load]);

  const create = useCallback(async (input: CreateThreadShareInput) => {
    const current = targetRef.current;
    if (!current) {
      fail(translate('workbench.relaySharingIsOnlyAvailableFromA'));
      return;
    }
    if ((input.workspaceAccess !== 'none' || input.scope === 'workspace') && !current.workspaceId) {
      fail(translate('workbench.thisThreadIsNotAttachedToA'));
      return;
    }
    const { deviceId, threadId, threadTitle, workspaceId, workspaceLabel } = current;
    const grant = {
      targetIdentifier: input.targetIdentifier, deviceId, workspaceScope: 'all' as const, workspaceIds: [],
      label: input.label ?? null, threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess,
      canCreateThreads: input.canCreateThreads ?? false,
    };
    await run(() => input.scope === 'device'
      ? createRelayGrant({ ...grant, scope: 'device' })
      // Every thread of this workspace; new threads only there.
      : input.scope === 'workspace'
        ? createRelayGrant({ ...grant, scope: 'workspace', workspaceId, workspaceLabel })
        : createRelayShare({
          targetIdentifier: input.targetIdentifier, deviceId, threadId, threadTitle,
          workspaceId: input.workspaceAccess === 'none' ? null : workspaceId,
          workspaceLabel: input.workspaceAccess === 'none' ? null : workspaceLabel,
          label: input.label ?? null, threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess,
        }), 'workbench.unableToCreateShare', true);
  }, [fail, run]);

  const update = useCallback((shareId: string, input: CreateThreadShareInput) => run(async () => {
    const current = targetRef.current;
    const context = contexts.current.get(shareId);
    if (!current || !context) throw new Error(translate('sharing.dialogIncomplete'));
    if (shareId.startsWith('grant:')) await updateRelayGrant(shareId.slice('grant:'.length), {
      threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess, label: input.label ?? null,
      ...(input.canCreateThreads === undefined ? {} : { canCreateThreads: input.canCreateThreads }),
    }, context);
    else await updateRelayShare(shareId, {
      threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess,
      workspaceId: current.workspaceId, workspaceLabel: current.workspaceLabel,
      label: input.label ?? null,
    }, context);
  }, 'workbench.unableToUpdatePermissions', false), [run]);

  // A failed revoke shows in the window; nothing waits on it.
  const revoke = useCallback((shareId: string) => run(
    () => shareId.startsWith('grant:') ? revokeRelayGrant(shareId.slice('grant:'.length)) : revokeRelayShare(shareId),
    'workbench.unableToRevokeShare', true,
  ).catch(() => {}), [run]);

  return { state, busy, load, create, update, revoke };
}

const NO_TURNS = { status: 'ready' as const, data: null, error: null };
const noop = () => {};

/** The thread page's share window, opened from a thread's menu: this thread,
    its workspace or the whole device. Only the device's owner opens it. */
export function ThreadShareDialog({ target, open, onClose }: {
  target: ThreadShareTarget | null;
  open: boolean;
  onClose: () => void;
}) {
  const shares = useThreadShares(target);
  const { load } = shares;
  const shown = open && target !== null;
  useEffect(() => { if (shown) void load(); }, [shown, load, target?.deviceId, target?.threadId]);
  return (
    <ThreadActionsDialog
      appearance="matter"
      initialMode="share"
      open={shown}
      busy={shares.busy}
      turnsState={NO_TURNS}
      shareAvailable
      shareState={shares.state}
      onCancel={() => { if (!shares.busy) onClose(); }}
      onLoadTurns={noop}
      onExport={noop}
      onCreateShare={shares.create}
      onUpdateShare={shares.update}
      onRevokeShare={shares.revoke}
      deviceShareAvailable
      workspaceShareAvailable={Boolean(target?.workspaceId)}
      workspaceLabel={target?.workspaceLabel ?? null}
    />
  );
}
