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

export interface StorageOverviewDto {
  deviceName: string;
  /** The last scan's page; null until a scan runs (only on request). */
  scan: StorageScanDto | null;
}

export interface StoragePruneResultDto {
  items: number;
  bytes: number;
}

export interface StorageCompactResultDto {
  beforeBytes: number;
  afterBytes: number;
}
