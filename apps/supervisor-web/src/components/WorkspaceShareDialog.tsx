import { translate } from '@pockymoe/thread-ui/i18n';
import {
  ThreadActionsDialog,
  type CreateThreadShareInput,
  type ThreadShareSummary,
} from '@pockymoe/thread-ui';
import type { WorkspaceDto } from '@pockymoe/shared';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  createRelayGrant,
  fetchRelayPortal,
  revokeRelayGrant,
  updateRelayGrant,
} from '../lib/api';
import { sharingEditInput, type SharingInput } from '../lib/signedSharing';
import { SignedShareDetails } from './SignedShareDetails';
import { useAppShellNav } from './AppShellNavContext';

type ShareState = { status: 'idle' | 'loading' | 'ready' | 'failed'; shares: ThreadShareSummary[]; error: string | null };

const NO_TURNS = { status: 'ready' as const, data: null, error: null };
const noop = () => {};

function message(caught: unknown, fallback: string) {
  return caught instanceof Error && caught.message ? caught.message : fallback;
}

/** The thread page's share window with one scope: this workspace on this
    device (docs/account-key-trust.md#sharing). Opened from a workspace's menu. */
export function WorkspaceShareDialog({ deviceId, workspace, onClose }: {
  deviceId: string;
  workspace: WorkspaceDto | null;
  onClose: () => void;
}) {
  const shell = useAppShellNav();
  const [state, setState] = useState<ShareState>({ status: 'idle', shares: [], error: null });
  const [busy, setBusy] = useState(false);
  const contexts = useRef(new Map<string, SharingInput>());
  const workspaceId = workspace?.id ?? null;

  const load = useCallback(async () => {
    if (!workspaceId) return;
    setState(current => ({ ...current, status: 'loading', error: null }));
    try {
      const portal = await fetchRelayPortal();
      const grants = (portal.grantsByMe ?? []).filter(grant =>
        grant.deviceId === deviceId && grant.scope === 'workspace' && grant.workspaceId === workspaceId);
      contexts.current = new Map(grants.map(grant => [`grant:${grant.id}`, sharingEditInput('grant', grant)] as const));
      setState({
        status: 'ready', error: null,
        shares: grants.map(grant => ({
          id: `grant:${grant.id}`, scope: 'workspace' as const,
          trustContent: <SignedShareDetails kind="grant" row={grant} />,
          targetUsername: grant.targetUsername, label: grant.label,
          threadAccess: grant.threadAccess, workspaceAccess: grant.workspaceAccess,
          canCreateThreads: grant.canCreateThreads,
          workspaceLabel: grant.workspaceLabel ?? workspace?.label ?? null, createdAt: grant.createdAt,
        })),
      });
    } catch (caught) {
      setState(current => ({ ...current, status: 'failed', error: message(caught, translate('workbench.unableToLoadActiveShares')) }));
    }
  }, [deviceId, workspaceId, workspace?.label]);

  useEffect(() => { void load(); }, [load]);

  // Every mutation reloads the list and reports its failure in the window.
  const run = useCallback(async (action: () => Promise<unknown>, fallback: Parameters<typeof translate>[0]) => {
    setBusy(true);
    setState(current => ({ ...current, error: null }));
    try {
      await action();
      await load();
    } catch (caught) {
      setState(current => ({ ...current, status: 'failed', error: message(caught, translate(fallback)) }));
      throw caught;
    } finally {
      setBusy(false);
    }
  }, [load]);

  const create = useCallback((input: CreateThreadShareInput) => run(() => createRelayGrant({
    targetIdentifier: input.targetIdentifier, deviceId, scope: 'workspace',
    workspaceId, workspaceLabel: workspace?.label ?? null, workspaceScope: 'all', workspaceIds: [],
    label: input.label ?? null, threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess,
    canCreateThreads: input.canCreateThreads ?? false,
  }), 'workbench.unableToCreateShare'), [deviceId, run, workspace?.label, workspaceId]);

  const update = useCallback((shareId: string, input: CreateThreadShareInput) => run(async () => {
    const context = contexts.current.get(shareId);
    if (!context) throw new Error(translate('sharing.dialogIncomplete'));
    await updateRelayGrant(shareId.slice('grant:'.length), {
      threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess, label: input.label ?? null,
      ...(input.canCreateThreads === undefined ? {} : { canCreateThreads: input.canCreateThreads }),
    }, context);
  }, 'workbench.unableToUpdatePermissions'), [run]);

  const revoke = useCallback((shareId: string) => run(
    () => revokeRelayGrant(shareId.slice('grant:'.length)),
    'workbench.unableToRevokeShare',
  ).catch(noop), [run]);

  // The window takes its light or dark look from the nearest thread UI shell,
  // which the Workspaces page does not otherwise have.
  return (
    <div className="thread-ui-shell" style={{ display: 'contents' }} data-theme-effective={shell?.effectiveTheme ?? 'light'} data-theme-mode={shell?.themeMode ?? 'system'}>
    <ThreadActionsDialog
      appearance="matter"
      initialMode="share"
      open={workspace !== null}
      busy={busy}
      turnsState={NO_TURNS}
      shareAvailable
      shareState={state}
      workspaceOnly
      workspaceLabel={workspace?.label ?? null}
      onCancel={() => { if (!busy) onClose(); }}
      onLoadTurns={noop}
      onExport={noop}
      onCreateShare={create}
      onUpdateShare={update}
      onRevokeShare={revoke}
    />
    </div>
  );
}
