import type { RelayEffectiveAccessDto } from '@pockymoe/shared';

type Access = RelayEffectiveAccessDto | null | undefined;

/** The built-in terminal plugin; recipients never see or switch it. */
export const TERMINAL_PLUGIN_ID = 'remote-codex.terminal';

/**
 * Mirrors `access::terminal_route`: terminals, shell commands and the terminal
 * plugin belong to the device owner. No share or grant, read or control, gets one.
 * Off a relay device route (local mode) the browser is the owner.
 */
export function canUseTerminal(access: Access, relayRoute: boolean) {
  return !relayRoute || access?.kind === 'owner';
}

/** A share of one workspace: its threads and, as allowed, its files. */
export function workspaceShare(access: Access): access is RelayEffectiveAccessDto & { scope: 'workspace'; workspaceId: string } {
  return access?.kind === 'shared' && access.scope === 'workspace' && Boolean(access.workspaceId);
}

/**
 * Thread and workspace shares stay inside their workspace: the file tree starts at
 * its root and no path may leave it (`..`, absolute or drive paths). The owner and
 * a whole-device share with write access may also open linked files outside it.
 */
export function filesConfinedToWorkspace(access: Access, relayRoute: boolean) {
  if (!relayRoute || access?.kind === 'owner') return false;
  return !(access?.kind === 'shared' && access.scope === 'device' && access.workspaceAccess === 'write');
}

/** A path a confined recipient may ask for: relative, inside the workspace root. */
export function pathInsideWorkspace(path: string) {
  const normalized = path.replace(/\\/g, '/');
  if (!normalized || normalized.startsWith('/') || /^[a-z]:/i.test(normalized) || normalized.includes('\0')) return false;
  return !normalized.split('/').some(part => part === '..');
}

/** May this recipient start threads, and where (`null`: anywhere on the device). */
export function newThreadScope(access: Access, relayRoute: boolean): { allowed: boolean; workspaceId: string | null } {
  if (!relayRoute || access?.kind === 'owner') return { allowed: true, workspaceId: null };
  if (access?.kind !== 'shared' || access.threadAccess !== 'control' || !access.canCreateThreads) return { allowed: false, workspaceId: null };
  if (access.scope === 'device') return { allowed: true, workspaceId: null };
  if (workspaceShare(access)) return { allowed: true, workspaceId: access.workspaceId };
  return { allowed: false, workspaceId: null };
}
