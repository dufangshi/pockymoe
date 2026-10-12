CREATE TABLE IF NOT EXISTS device_owner_trust (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  relay_device_id TEXT,
  strict INTEGER NOT NULL DEFAULT 0 CHECK (strict IN (0, 1)),
  account_key_id TEXT,
  account_public_key TEXT,
  enroll_secret BLOB CHECK (enroll_secret IS NULL OR length(enroll_secret) = 32),
  bound_at INTEGER,
  web_origin TEXT
);
INSERT OR IGNORE INTO device_owner_trust(singleton) VALUES (1);
CREATE TABLE IF NOT EXISTS browser_registrations (
  browser_id TEXT PRIMARY KEY,
  public_key TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('owner', 'grantee')),
  account_key_id TEXT NOT NULL,
  user_id TEXT,
  registered_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS account_revocations (
  account_key_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('browser', 'device')),
  subject_id TEXT NOT NULL,
  revocation_json TEXT NOT NULL,
  PRIMARY KEY (account_key_id, kind, subject_id)
);
CREATE TABLE IF NOT EXISTS device_certificates (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  certificate_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS owner_port_mappings (
  mapping_id TEXT PRIMARY KEY,
  browser_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS device_grants (
  grant_id TEXT PRIMARY KEY,
  grant_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS peer_certificates (
  device_id TEXT PRIMARY KEY,
  identity_key TEXT NOT NULL,
  account_key_id TEXT NOT NULL
);
