import { appUrl } from './webBase';
import { translate } from '@pockymoe/thread-ui/i18n';
import { pinInviteOwner } from './sharedDeviceTrust';
export interface SharedInvite {
  deviceId: string;
  ownerAccountKeyId: string;
  threadId?: string;
  /** A shared workspace: the invite opens its threads. */
  workspaceId?: string;
}
export function parseSharedInvite(fragment: string): SharedInvite {
  const params = new URLSearchParams(fragment.replace(/^#/, ''));
  if (
    params.get('v') !== '2' ||
    !params.get('device') ||
    !/^[A-Za-z0-9_-]{43}$/.test(params.get('owner') ?? '') ||
    ['v', 'device', 'owner', 'thread', 'workspace'].some(
      (k) => params.getAll(k).length > 1,
    ) ||
    (params.has('thread') && !params.get('thread')) ||
    (params.has('workspace') && !params.get('workspace')) ||
    (params.has('thread') && params.has('workspace'))
  )
    throw new Error(translate('sharing.inviteInvalid'));
  return {
    deviceId: params.get('device')!,
    ownerAccountKeyId: params.get('owner')!,
    ...(params.get('thread') ? { threadId: params.get('thread')! } : {}),
    ...(params.get('workspace') ? { workspaceId: params.get('workspace')! } : {}),
  };
}
export function sharedInviteUrl(origin: string, invite: SharedInvite) {
  const params = new URLSearchParams({
    v: '2',
    device: invite.deviceId,
    owner: invite.ownerAccountKeyId,
  });
  if (invite.threadId) params.set('thread', invite.threadId);
  else if (invite.workspaceId) params.set('workspace', invite.workspaceId);
  return `${origin}${appUrl('shared')}#${params}`;
}
export async function acceptSharedInvite(invite: SharedInvite) {
  await pinInviteOwner(invite.deviceId, invite.ownerAccountKeyId);
  const device = `/devices/${encodeURIComponent(invite.deviceId)}`;
  if (invite.threadId) return `${device}/threads/${encodeURIComponent(invite.threadId)}`;
  if (invite.workspaceId) return `${device}/threads?workspaceId=${encodeURIComponent(invite.workspaceId)}`;
  return `${device}/workspaces`;
}
