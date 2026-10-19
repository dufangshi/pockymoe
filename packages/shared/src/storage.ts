// Mirrors crates/protocol/src/storage.rs.

/** What a cleanup removes from a command's text. */
export type CommandTextCleanup = 'keep' | 'truncate' | 'remove';

/** What a cleanup removes from turns that ended (never from running ones). */
export interface StorageCleanupDto {
  /** Command output (and the command's input details). */
  commandOutput: boolean;
  commandText: CommandTextCleanup;
  /** Characters kept by `truncate`. */
  commandTextChars: number;
  /** The diffs of file changes; their paths and line counts stay. */
  fileDiffs: boolean;
}

/** Automatic cleanup: one age for everything it removes. null turns it off. */
export interface StorageSettingsDto extends StorageCleanupDto {
  retentionDays: number | null;
}

/** One conversation: a root thread with its agent threads. */
export interface StorageThreadDto {
  threadId: string;
  title: string;
  workspaceLabel: string | null;
  bytes: number;
  commandBytes: number;
  /** Part held by file changes (their diffs). */
  fileChangeBytes: number;
  items: number;
  agentThreads: number;
  lastActivityAt: string | null;
}

export interface StorageScanDto {
  scannedAt: string;
  deviceName: string;
  databaseBytes: number;
  freeBytes: number;
  historyBytes: number;
  commandBytes: number;
  /** Part held by file changes (their diffs). */
  fileChangeBytes: number;
  prunedItems: number;
  prunedBytes: number;
  threadCount: number;
  conversationCount: number;
  threads: StorageThreadDto[];
  offset: number;
  nextOffset: number | null;
}

/** A cleanup or compaction running in the background, or the last one. */
export interface StorageJobDto {
  /** `dedupe` is the one-time removal of input copies of commands. */
  action: 'prune' | 'dedupe' | 'compact';
  state: 'running' | 'done' | 'failed';
  /** Items to look at (prune); 0 until counted, and for compaction. */
  total: number;
  processed: number;
  /** Items changed and bytes removed so far (prune). */
  items: number;
  bytes: number;
  beforeBytes: number | null;
  afterBytes: number | null;
  startedAt: string;
  finishedAt: string | null;
  error: string | null;
  /** The conversation a manual cleanup targets; null for the whole device. */
  threadId?: string | null;
}

export interface StorageOverviewDto {
  deviceName: string;
  /** The last scan's page; null until a scan runs (only on request). */
  scan: StorageScanDto | null;
  /** The running or last cleanup/compaction since the Supervisor started. */
  job?: StorageJobDto | null;
}
