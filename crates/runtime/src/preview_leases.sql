CREATE TABLE IF NOT EXISTS preview_leases (
  lease_id TEXT PRIMARY KEY,
  mapping_id TEXT NOT NULL,
  browser_id TEXT,
  account_key_id TEXT,
  expires_at INTEGER NOT NULL,
  CHECK ((browser_id IS NULL) = (account_key_id IS NULL))
);
CREATE INDEX IF NOT EXISTS preview_leases_mapping_idx ON preview_leases(mapping_id);
CREATE INDEX IF NOT EXISTS preview_leases_browser_idx ON preview_leases(account_key_id, browser_id);
