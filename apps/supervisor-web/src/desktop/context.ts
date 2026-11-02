import { createContext, useContext } from 'react';
import type { MatterWorkbenchOptions } from '@pockymoe/thread-ui';
export interface NavigationActions { currentKey: string; threads?: readonly import('@pockymoe/thread-ui').WorkbenchThread[]; onToggleThreadFavorite(key: string): Promise<void>; onThreadRenamed(key: string, title: string): Promise<void>; onThreadRemoved(key: string): Promise<void> }
export interface DesktopSurfaceContextValue {
  multiWindow: boolean;
  routeReady(path: string): void;
  workspaceControlsEnabled?: boolean;
  openMap(): void;
  revealInMap(key: string): void;
  renderActivity(navigation: NavigationActions): MatterWorkbenchOptions['sidebarLeading'];
  railLeading: MatterWorkbenchOptions['railLeading'];
  railBottomLeading?: MatterWorkbenchOptions['railBottomLeading'];
}
export const DesktopSurfaceContext = createContext<DesktopSurfaceContextValue | null>(null);
export const useDesktopSurface = () => useContext(DesktopSurfaceContext);
export function useDesktopWorkbench(navigation: NavigationActions) {
  const surface = useDesktopSurface();
  return surface ? { workspaceControlsEnabled: surface.workspaceControlsEnabled !== false, sidebarLeading: surface.renderActivity(navigation), railLeading: surface.railLeading, railBottomLeading: surface.railBottomLeading } : {};
}

export function hasWorkspaceContext(path: string, search = '') {
  path = path.replace(/\/+$/, '') || '/';
  const thread = path.match(/^\/(?:devices\/[^/]+\/)?threads\/([^/]+)$/)?.[1];
  if (thread) return !['new', 'import'].includes(thread);
  return /^\/(?:devices\/[^/]+\/)?threads$/.test(path) && Boolean(new URLSearchParams(search).get('workspaceId'));
}

/** Default true for ordinary Web/mobile routes; only a retained desktop outlet pauses. */
export const RetainedRouteActiveContext = createContext(true);
export const useRetainedRouteActive = () => useContext(RetainedRouteActiveContext);
