// Mirrors crates/protocol/src/harness_resources.rs.

/** Release policy; null disables the rule. */
export interface HarnessResourceSettingsDto {
  idleReleaseMinutes: number | null;
  maxLoadedThreads: number | null;
  maxHarnessMemoryBytes: number | null;
}

export type ProcessMemoryMetric = 'pss' | 'physFootprint' | 'privateWorkingSet' | 'rss';

export type HarnessProcessRole = 'adapter' | 'agent' | 'bridge' | 'mcp' | 'tool' | 'other';

export interface HarnessProcessDto {
  pid: number;
  role: HarnessProcessRole;
  name: string;
  bytes: number | null;
}

export type HarnessLoadReason =
  | 'view'
  | 'create'
  | 'turn'
  | 'queue'
  | 'automation'
  | 'resume'
  | 'fork'
  | 'other';

export type HarnessBusyReason =
  | 'turn'
  | 'pendingRequest'
  | 'queuedInput'
  | 'backgroundTask'
  | 'subagent'
  | 'goal';

export interface LoadedThreadDto {
  threadId: string;
  title: string;
  workspaceId: string;
  provider: string;
  agentId: string | null;
  loadReason: HarnessLoadReason;
  loadedAt: string;
  lastActivityAt: string;
  displayed: boolean;
  /** Empty when releasable. */
  busy: HarnessBusyReason[];
  sharesProcessWith: string[];
  rootPid: number | null;
  bytes: number | null;
  processes: HarnessProcessDto[];
}

/** `GET /api/device/processes` */
export interface HarnessProcessesDto {
  sampledAt: string;
  metric: ProcessMemoryMetric;
  supervisorPid: number;
  supervisorBytes: number | null;
  harnessBytes: number | null;
  totalBytes: number | null;
  settings: HarnessResourceSettingsDto;
  overThreadLimit: boolean;
  overMemoryLimit: boolean;
  threads: LoadedThreadDto[];
  other: HarnessProcessDto[];
}

/** `POST /api/threads/{id}/release` */
export interface ReleaseThreadInput {
  interrupt?: boolean;
}

export interface ReleaseThreadResultDto {
  threadId: string;
  released: boolean;
  alsoReleased: string[];
}

/** WebSocket client message: the threads this page displays right now. */
export interface ThreadViewMessage {
  type: 'thread.view';
  threadIds: string[];
}
