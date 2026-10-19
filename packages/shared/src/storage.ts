// Mirrors crates/protocol/src/storage.rs.

/** null keeps command output forever. */
export interface StorageSettingsDto {
  commandOutputRetentionDays: number | null;
}

/** One conversation: a root thread with its agent threads. */
export interface StorageThreadDto {
  threadId: string;
  title: string;
  workspaceLabel: string | null;
  bytes: number;
  commandBytes: number;
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
  action: 'prune' | 'compact';
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
}

export interface StorageOverviewDto {
  deviceName: string;
  /** The last scan's page; null until a scan runs (only on request). */
  scan: StorageScanDto | null;
  /** The running or last cleanup/compaction since the Supervisor started. */
  job?: StorageJobDto | null;
}
