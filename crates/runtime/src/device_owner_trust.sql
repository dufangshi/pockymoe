CREATE TABLE IF NOT EXISTS device_owner_trust (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  relay_device_id TEXT,
  strict INTEGER NOT NULL DEFAULT 0 CHECK (strict IN (0, 1))
);
INSERT OR IGNORE INTO device_owner_trust(singleton) VALUES (1);
CREATE TABLE IF NOT EXISTS owner_credentials (
  credential_id TEXT PRIMARY KEY,
  record_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS owner_sessions (
  session_id TEXT PRIMARY KEY,
  public_key TEXT NOT NULL,
  credential_id TEXT NOT NULL REFERENCES owner_credentials(credential_id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS pairing_offers (
  pairing_id TEXT PRIMARY KEY,
  secret BLOB NOT NULL CHECK (length(secret) = 32),
  web_origin TEXT NOT NULL,
  rp_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used_at INTEGER,
  failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS device_certificates (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  certificate_json TEXT NOT NULL
);
-- Keep provenance after credential revocation so reset cannot revive a mapping.
CREATE TABLE IF NOT EXISTS owner_port_mappings (
  mapping_id TEXT PRIMARY KEY,
  credential_id TEXT NOT NULL
);

-- Device-authorized shares; relay rows are only references, never authority.
CREATE TABLE IF NOT EXISTS device_grants (
  grant_id TEXT PRIMARY KEY,
  grant_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS grantee_sessions (
  session_id TEXT PRIMARY KEY,
  public_key TEXT NOT NULL,
  credential_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS peer_certificates (
  device_id TEXT PRIMARY KEY,
  identity_key TEXT NOT NULL,
  credential_id TEXT NOT NULL REFERENCES owner_credentials(credential_id) ON DELETE CASCADE
);
