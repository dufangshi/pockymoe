import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayAccessGrantDto, RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';
import { Link } from 'react-router-dom';
import './signed-share.css';

/** One of my shares that lets someone control agents on a device. */
export interface ControlShare {
  key: string;
  scope: 'thread' | 'workspace' | 'device';
  threadTitle: string | null;
  workspaceLabel: string | null;
  recipient: string;
  threadAccess: 'read' | 'control';
  canCreateThreads: boolean;
}

const clean = (value: string | null | undefined) => value?.trim() || null;

/** The shares that make the relay refuse a device as a peer source, by the
    relay's rule (`peer::device_shared_for_control`): active (not waiting for
    the safety-code check), with control or permission to start threads. */
export function controlShares(portal: RelayPortalSummaryDto | null, deviceId: string): ControlShare[] {
  if (!portal) return [];
  const live = (row: RelaySessionShareDto | RelayAccessGrantDto) =>
    row.deviceId === deviceId && row.pending !== true && row.ownerUserId !== row.targetUserId;
  return [
    ...portal.sharedByMe.filter(row => live(row) && row.threadAccess === 'control').map(row => ({
      key: `share:${row.id}`,
      scope: 'thread' as const,
      threadTitle: clean(row.threadTitle) ?? clean(row.label),
      workspaceLabel: clean(row.workspaceLabel),
      recipient: row.targetUsername,
      threadAccess: row.threadAccess,
      canCreateThreads: false,
    })),
    ...(portal.grantsByMe ?? []).filter(row => live(row) && (row.threadAccess === 'control' || row.canCreateThreads)).map(row => ({
      key: `grant:${row.id}`,
      scope: row.scope,
      threadTitle: clean(row.threadTitle) ?? clean(row.label),
      workspaceLabel: clean(row.workspaceLabel),
      recipient: row.targetUsername,
      threadAccess: row.threadAccess,
      canCreateThreads: row.canCreateThreads,
    })),
  ];
}

function what(share: ControlShare) {
  if (share.scope === 'device') return translate('devices.sharedControlDevice');
  if (share.scope === 'workspace') return translate('devices.sharedControlWorkspace', { workspace: share.workspaceLabel ?? '—' });
  const title = share.threadTitle ?? '—';
  return share.workspaceLabel
    ? translate('devices.sharedControlThread', { title, workspace: share.workspaceLabel })
    : translate('devices.sharedControlThreadAlone', { title });
}

function access(share: ControlShare) {
  return [
    translate(share.threadAccess === 'control' ? 'devices.collaborator' : 'devices.viewOnly'),
    ...(share.canCreateThreads ? [translate('sharing.newThreadsShort')] : []),
  ].join(', ');
}

/** Why a device's agents cannot reach my other devices: each share that causes
    it, and what to change. Only the owner sees this list. */
export function SharedControlReasons({ shares }: { shares: ControlShare[] }) {
  useI18n();
  return (
    <div className="shared-control-reasons">
      <p>{translate('devices.sharedControlDetail')}</p>
      {shares.length ? (
        <>
          <p className="shared-control-heading">{translate('devices.sharedControlBecause')}</p>
          <ul>
            {shares.map(share => (
              <li key={share.key}>
                <span className="shared-control-what">{what(share)}</span>
                <span className="shared-control-who">{translate('devices.sharedControlWith', { name: share.recipient, access: access(share) })}</span>
              </li>
            ))}
          </ul>
          <p>{translate('devices.sharedControlFix')}</p>
        </>
      ) : null}
      <Link className="shared-control-link" to="/relay-devices?tab=shared&dir=out">{translate('devices.sharedControlOpen')}</Link>
    </div>
  );
}
