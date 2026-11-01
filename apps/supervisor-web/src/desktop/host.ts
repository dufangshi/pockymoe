// Mirrors /Users/mac/dev/remote-codex-app/desktop/src/shared/contract.ts, bridge v1.
// Single source of truth for the desktop host bridge. supervisor-web mirrors the
// relay-page half in apps/supervisor-web/src/desktop/host.ts; bump
// HOST_BRIDGE_VERSION on any breaking change and feature-detect, never assume.
// Design: docs/desktop-design.zh.md §6.

export const HOST_BRIDGE_VERSION = 1;

export interface HostCapabilities {
  surface: 'desktop';
  bridgeVersion: number;
  platform: 'darwin' | 'win32' | 'linux';
  shellVersion: string;
  notifications: 'local' | 'local+apns';
  windowChrome: {
    kind: 'hiddenInset' | 'titleBarOverlay';
    /** Horizontal space the page must leave for native window controls. */
    trafficLightsWidth: number;
    toolbarHeight: number;
    /** Windows: controls sit on the right instead of the left. */
    controlsSide: 'left' | 'right';
  };
  features: {
    pet: boolean;
    quickWindow: boolean;
    multiWindow: boolean;
    nativeSave: boolean;
    badge: boolean;
  };
}

export type ActivityState = 'needsInput' | 'failed' | 'completed' | 'running' | 'idle';

export interface ActivityRequest {
  id: string;
  kind: 'approval' | 'input';
  /** Plain text, at most 140 characters. */
  summary: string;
}

export interface ActivityItem {
  /** `${deviceId}:${threadId}` */
  key: string;
  deviceId: string;
  deviceName: string;
  workspaceId: string;
  workspaceLabel: string;
  threadId: string;
  title: string;
  agentName?: string | null;
  state: ActivityState;
  /** ISO time the item entered its current state. */
  since: string;
  turnId?: string | null;
  unread: boolean;
  request?: ActivityRequest | null;
  /** First line of the result for completed/failed, at most 140 characters. */
  resultSummary?: string | null;
  durationMs?: number | null;
}

export interface ActivitySnapshot {
  version: 1;
  generatedAt: string;
  accountId: string;
  /** Thread the focused main window is showing, or null. */
  focusedThreadKey: string | null;
  /** needsInput, failed (unread, last 24h), completed (unread) and running only. */
  items: ActivityItem[];
}

export type HostCommand =
  | { id: string; type: 'navigate'; path: string }
  | { id: string; type: 'markRead'; key: string }
  | {
      id: string;
      type: 'resolveRequest';
      deviceId: string;
      threadId: string;
      requestId: string;
      decision: 'approve' | 'deny';
    }
  | { id: string; type: 'reply'; deviceId: string; threadId: string; text: string }
  | { id: string; type: 'openPalette' }
  | { id: string; type: 'newThread' }
  | { id: string; type: 'openDevices' };

export interface HostCommandResult {
  ok: boolean;
  error?: string;
}

export interface DesktopSettings {
  relayOrigin: string;
  alertMode: 'petFirst' | 'system' | 'both';
  petVisible: boolean;
  petId: string;
  petSize: 'S' | 'M' | 'L';
  petMini: boolean;
  petSound: boolean;
  launchAtLogin: boolean;
  /** Electron accelerator; default 'Alt+Space'. */
  quickWindowShortcut: string;
  snoozeUntil: string | null;
}

/** Exposed as window.remoteCodexHost to the configured relay origin's top frame only. */
export interface RemoteCodexHost {
  capabilities(): Promise<HostCapabilities>;
  activity: { sync(snapshot: ActivitySnapshot): void };
  onCommand(listener: (command: HostCommand) => void): () => void;
  commandResult(id: string, result: HostCommandResult): void;
  window: {
    openThread(path: string, options?: { newWindow?: boolean }): void;
    setTitle(title: string): void;
  };
  shell: { openExternal(url: string): void };
  files: {
    saveAs(name: string, data: ArrayBuffer): Promise<{ saved: boolean; path?: string }>;
  };
  pet: { setVisible(visible: boolean): void; isVisible(): Promise<boolean> };
  settings: {
    get(): Promise<DesktopSettings>;
    set(patch: Partial<DesktopSettings>): Promise<DesktopSettings>;
  };
  badge: { set(count: number, style: 'attention' | 'running' | 'none'): void };
}


declare global { interface Window { remoteCodexHost?: RemoteCodexHost } }
