import { translate } from '@pockymoe/thread-ui/i18n';
import { ArrowRight, Eye, Lock, LockOpen, Pencil, Plus, Share2, Trash2 } from 'lucide-react';
import type { ManagedAction } from './ManagedObjectMenu';

export type WorkspaceManageAction = 'rename' | 'share' | 'isolate' | 'path' | 'remove';
export type WorkspaceActionId = 'open' | 'new' | WorkspaceManageAction;

export interface WorkspaceActionOptions {
  navigate(path: string): void;
  /** The workspace's threads and its new-thread page, on the device the page shows. */
  openHref: string;
  newThreadHref: string;
  /** Rename, share, the path and removal open their window in place; isolation switches at once. */
  manage(action: WorkspaceManageAction): void;
  /** Only the device's owner shares its workspaces, through the relay. */
  canShare?: boolean;
}

/**
 * A workspace's actions, in the order shared with `deviceActions`: open and
 * start work, then rename / share / isolate, then its path (where a device has
 * its settings), and removal last, behind a separator. Isolation is a switch
 * here, not a window: the state is already known and nothing else depends on it
 * (docs/agent-isolation.md); a runtime that predates it offers none.
 */
export function workspaceActions(workspace: { agentIsolated?: boolean | undefined }, options: WorkspaceActionOptions): ManagedAction[] {
  return [
    { id: 'open', label: translate('devices.manageOpen'), icon: <ArrowRight />, onSelect: () => options.navigate(options.openHref) },
    { id: 'new', label: translate('devices.manageNewThread'), icon: <Plus />, onSelect: () => options.navigate(options.newThreadHref) },
    { id: 'rename', label: translate('devices.manageRename'), icon: <Pencil />, onSelect: () => options.manage('rename') },
    ...(options.canShare ? [{ id: 'share', label: translate('devices.manageShare'), icon: <Share2 />, onSelect: () => options.manage('share') }] : []),
    ...(workspace.agentIsolated === undefined ? [] : [{
      id: 'isolate',
      label: translate(workspace.agentIsolated ? 'devices.manageStopIsolating' : 'devices.manageIsolateNow'),
      icon: workspace.agentIsolated ? <LockOpen /> : <Lock />,
      onSelect: () => options.manage('isolate'),
    }]),
    { id: 'path', label: translate('devices.manageViewPath'), icon: <Eye />, onSelect: () => options.manage('path') },
    { id: 'remove', label: translate('devices.manageRemove'), icon: <Trash2 />, onSelect: () => options.manage('remove'), separator: true, tone: 'danger' },
  ];
}
