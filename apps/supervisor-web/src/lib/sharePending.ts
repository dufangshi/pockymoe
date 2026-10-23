import type { RelayPendingShareDto } from '@pockymoe/shared';

/** Shared access that waits for the owner's safety-code check
    (docs/account-key-trust.md#safety-code-check). The relay refuses everything
    under it with `share_pending`; whichever request meets that refusal first
    reports it, and the app shows the recipient their safety code. */
export const SHARE_PENDING_EVENT = 'pockymoe-share-pending';
/** An owner changed a share (approved, signed): pages showing the portal reload it. */
export const SHARING_CHANGED_EVENT = 'pockymoe-sharing-changed';
export const SHARE_PENDING_PATH = '/shared-pending';

const text = (value: unknown) => typeof value === 'string' ? value : null;

/** The refusal's `share`, checked field by field; null for anything else. */
export function pendingShare(value: unknown): RelayPendingShareDto | null {
  if (!value || typeof value !== 'object') return null;
  const share = value as Record<string, unknown>;
  const kind = share.kind === 'share' || share.kind === 'grant' ? share.kind : null;
  const scope = share.scope === 'thread' || share.scope === 'workspace' || share.scope === 'device' ? share.scope : null;
  const id = text(share.id), deviceId = text(share.deviceId);
  if (!kind || !scope || !id || !deviceId) return null;
  return {
    kind,
    id,
    scope,
    deviceId,
    ownerUsername: text(share.ownerUsername),
    deviceName: text(share.deviceName),
    threadId: text(share.threadId),
    threadTitle: text(share.threadTitle),
    workspaceId: text(share.workspaceId),
    workspaceLabel: text(share.workspaceLabel),
    label: text(share.label),
    threadAccess: share.threadAccess === 'control' ? 'control' : 'read',
    workspaceAccess: share.workspaceAccess === 'write' ? 'write' : share.workspaceAccess === 'read' ? 'read' : 'none',
  };
}

/** Reads a relay error body; reports and returns the pending share it names. */
export function reportSharePendingBody(body: unknown) {
  const payload = body && typeof body === 'object' ? body as Record<string, unknown> : null;
  if (payload?.code !== 'share_pending') return null;
  const share = pendingShare(payload.share);
  // The service worker shares this code and has no window to tell.
  if (share && typeof window !== 'undefined') window.dispatchEvent(new CustomEvent<RelayPendingShareDto>(SHARE_PENDING_EVENT, { detail: share }));
  return share;
}

export function sharePendingHref(share: Pick<RelayPendingShareDto, 'kind' | 'id'>) {
  return `${SHARE_PENDING_PATH}?${new URLSearchParams({ kind: share.kind, id: share.id }).toString()}`;
}

export function onSharePending(listener: (share: RelayPendingShareDto) => void) {
  const handle = (event: Event) => listener((event as CustomEvent<RelayPendingShareDto>).detail);
  window.addEventListener(SHARE_PENDING_EVENT, handle);
  return () => window.removeEventListener(SHARE_PENDING_EVENT, handle);
}

export function announceSharingChanged() {
  window.dispatchEvent(new Event(SHARING_CHANGED_EVENT));
}

/** What people type: case, spaces and dashes do not matter; O reads as 0 and
    I/L as 1 (as the relay compares). */
export function normalizeSafetyCode(code: string) {
  return code.toUpperCase().replace(/[\s-]/g, '').replace(/O/g, '0').replace(/[IL]/g, '1');
}
