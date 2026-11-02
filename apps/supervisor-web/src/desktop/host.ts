// Mirrors desktop/src/shared/contract.ts in dufangshi/pockymoe-app, bridge v2.
// Single source of truth for the desktop host bridge. supervisor-web mirrors the
// page half in apps/supervisor-web/src/desktop/host.ts; bump
// HOST_BRIDGE_VERSION on any breaking change and feature-detect, never assume.
// Design: docs/desktop-design.zh.md §6.

export const HOST_BRIDGE_VERSION = 2;

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

export type ActivityState = 'needsInput' | 'failed' | 'interrupted' | 'completed' | 'running' | 'idle';

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
  /**
   * For an agent thread: `${deviceId}:${rootThreadId}` of its conversation, and that
   * root's title. Counts and lists group by `rootThreadKey ?? key` (ui-principles §7:
   * an agent thread never appears on its own); approvals and replies still target `key`.
   */
  rootThreadKey?: string | null;
  rootThreadTitle?: string | null;
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
  /** needsInput, failed/interrupted (unread, last 24h), completed (unread) and running only. */
  items: ActivityItem[];
  /** Threads opened most recently (up to 5), for surfaces with no active items to show. */
  recent?: ActivityRecent[];
  /** Devices whose first read finished since this page connected; older items on them are history, not news. */
  syncedDeviceIds?: string[];
}

export interface ActivityRecent {
  key: string;
  deviceId: string;
  deviceName: string;
  workspaceLabel: string;
  threadId: string;
  title: string;
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
  | { id: string; type: 'revealInMap'; key: string }
  | { id: string; type: 'openMap' }
  | { id: string; type: 'newThread' }
  | { id: string; type: 'openDevices' }
  /** Text of a finished turn, for spoken summaries. Answers in `data`. */
  | { id: string; type: 'turnText'; deviceId: string; threadId: string; turnId?: string | null }
  /** Saved upstream connections on every connected device. Answers in `data`. */
  | { id: string; type: 'listConnections' }
  /** One completion through a saved connection; the key stays on that device. Answers in `data`. */
  | { id: string; type: 'completeWithConnection'; deviceId: string; connectionId: string; model: string; system: string; user: string };

export interface HostCommandResult {
  ok: boolean;
  /** Localized, user-facing reason. */
  error?: string;
  /** Machine-readable reason; 'needsThread' means the request must be answered inside the thread. */
  code?: 'needsThread';
  /** Result of a command that asks for data rather than performing an action. */
  data?: unknown;
}

export interface DesktopSettings {
  /** Origin of the Web UI the main window loads (GitHub Pages in production). */
  webOrigin: string;
  /** Origin of the relay API (APNs registration, health). Equal to webOrigin for same-origin relays. */
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

export interface Appearance {
  effective: 'light' | 'dark';
  /** Theme pack id: 'classic' | 'plum-pocket' | 'terminal' | … */
  pack: string;
  /** Resolved accent color (any CSS color) so local windows can match the page. */
  accent: string;
  fontScale?: number;
  /** The page's UI language (BCP 47, e.g. 'zh-CN', 'en'); local windows, tray, menus and notifications use it. */
  locale?: string;
}

/** Exposed as window.pockymoeHost to the configured webOrigin's top frame only. */
export interface PockymoeHost {
  capabilities(): Promise<HostCapabilities>;
  activity: { sync(snapshot: ActivitySnapshot): void };
  onCommand(listener: (command: HostCommand) => void): () => void;
  commandResult(id: string, result: HostCommandResult): void;
  window: {
    openThread(path: string, options?: { newWindow?: boolean }): void;
    setTitle(title: string): void;
    /** Opens the shell's native Preferences window (system notifications, pet, shortcuts). */
    openPreferences(section?: 'general' | 'pet' | 'shortcuts'): void;
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
  appearance: { sync(appearance: Appearance): void };
}


declare global { interface Window { pockymoeHost?: PockymoeHost } }

export const desktopHost = () => window.pockymoeHost?.capabilities ? window.pockymoeHost : null;
