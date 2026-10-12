import { translate } from '@pockymoe/thread-ui/i18n';
import { pinInviteIdentity } from './ownerDeviceTrust';
export interface SharedInvite {
  deviceId: string;
  identity: string;
  threadId?: string;
}
export function parseSharedInvite(fragment: string): SharedInvite {
  const params = new URLSearchParams(fragment.replace(/^#/, ''));
  if (
    params.get('v') !== '1' ||
    !params.get('device') ||
    !/^[A-Za-z0-9_-]{43}$/.test(params.get('identity') ?? '') ||
    ['v', 'device', 'identity', 'thread'].some(
      (k) => params.getAll(k).length > 1,
    ) ||
    (params.has('thread') && !params.get('thread'))
  )
    throw new Error(translate('sharing.inviteInvalid'));
  return {
    deviceId: params.get('device')!,
    identity: params.get('identity')!,
    ...(params.get('thread') ? { threadId: params.get('thread')! } : {}),
  };
}
export function sharedInviteUrl(origin: string, invite: SharedInvite) {
  const params = new URLSearchParams({
    v: '1',
    device: invite.deviceId,
    identity: invite.identity,
  });
  if (invite.threadId) params.set('thread', invite.threadId);
  return `${origin}/shared#${params}`;
}
export async function acceptSharedInvite(invite: SharedInvite) {
  await pinInviteIdentity(invite.deviceId, invite.identity);
  return `/devices/${encodeURIComponent(invite.deviceId)}/${invite.threadId ? `threads/${encodeURIComponent(invite.threadId)}` : 'workspaces'}`;
}
