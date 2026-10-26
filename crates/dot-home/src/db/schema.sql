-- Familiar schema, version 1. Times are unix seconds (UTC).

CREATE TABLE users (
  id            TEXT PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
  display_name  TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  role          TEXT NOT NULL CHECK (role IN ('admin', 'member')),
  totp_secret   BLOB,                -- sealed with the master key; NULL = not enrolled
  totp_pending  BLOB,                -- sealed; enrollment awaiting confirmation
  totp_last_step INTEGER NOT NULL DEFAULT 0, -- refuses reuse of a code
  disabled      INTEGER NOT NULL DEFAULT 0,
  max_instances INTEGER,             -- NULL = server default
  failed_logins INTEGER NOT NULL DEFAULT 0,
  locked_until  INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL,
  last_seen_at  INTEGER
);

CREATE TABLE sessions (
  id_hash      BLOB PRIMARY KEY,     -- sha256 of the cookie value
  public_id    TEXT NOT NULL UNIQUE, -- shown in the sessions list
  user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  csrf         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  confirmed_until INTEGER NOT NULL DEFAULT 0, -- re-authentication for sensitive actions
  ip           TEXT,
  user_agent   TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);

CREATE TABLE login_challenges (
  id_hash    BLOB PRIMARY KEY,
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL,
  attempts   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE invites (
  id         TEXT PRIMARY KEY,
  code_hash  BLOB NOT NULL UNIQUE,
  role       TEXT NOT NULL CHECK (role IN ('admin', 'member')),
  reset_user TEXT REFERENCES users(id) ON DELETE CASCADE, -- password reset link for this user
  created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used_at    INTEGER
);

CREATE TABLE security_events (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,
  kind       TEXT NOT NULL,
  ip         TEXT,
  user_agent TEXT,
  detail     TEXT
);
CREATE INDEX security_events_user ON security_events(user_id, at DESC);

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE instances (
  id            TEXT PRIMARY KEY,
  user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name          TEXT NOT NULL,
  driver        TEXT NOT NULL CHECK (driver IN ('qemu', 'remote')),
  cpus          INTEGER NOT NULL,
  memory_mb     INTEGER NOT NULL,
  disk_gb       INTEGER NOT NULL,
  port          INTEGER UNIQUE,      -- qemu: host port forwarded to the guest agent
  remote_host   TEXT,                -- remote: host:port
  secret        BLOB NOT NULL,       -- sealed guest secret
  desired       TEXT NOT NULL DEFAULT 'stopped' CHECK (desired IN ('running', 'stopped')),
  paused        INTEGER NOT NULL DEFAULT 0,
  sleep_after_minutes     INTEGER NOT NULL DEFAULT 30,
  hibernate_after_minutes INTEGER NOT NULL DEFAULT 240,
  guest_info    TEXT,                -- last GuestInfo JSON
  apps_cache    TEXT,                -- last installed-apps JSON
  is_builder    INTEGER NOT NULL DEFAULT 0, -- golden-image builder (admin only)
  created_at    INTEGER NOT NULL,
  last_active_at INTEGER,
  booted_at     INTEGER
);
CREATE INDEX instances_user ON instances(user_id);

CREATE TABLE permissions (
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  grp         TEXT NOT NULL,
  mode        TEXT NOT NULL CHECK (mode IN ('allow', 'ask', 'deny')),
  PRIMARY KEY (instance_id, grp)
);

CREATE TABLE enabled_packs (
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  pack_id     TEXT NOT NULL,
  enabled     INTEGER NOT NULL,
  PRIMARY KEY (instance_id, pack_id)
);

CREATE TABLE custom_packs (
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  pack_id     TEXT NOT NULL,
  manifest    TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (instance_id, pack_id)
);

CREATE TABLE activity (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  at          INTEGER NOT NULL,
  tool        TEXT NOT NULL,
  outcome     TEXT NOT NULL CHECK (outcome IN ('ok', 'error', 'denied', 'pending')),
  duration_ms INTEGER NOT NULL DEFAULT 0,
  detail      TEXT,
  error_code  TEXT,
  client      TEXT
);
CREATE INDEX activity_instance ON activity(instance_id, id DESC);

CREATE TABLE approvals (
  id          TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  tool        TEXT NOT NULL,
  summary     TEXT NOT NULL,
  args_hash   BLOB NOT NULL,         -- an approval covers exactly these arguments
  client      TEXT,
  status      TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'used')),
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL
);
CREATE INDEX approvals_instance ON approvals(instance_id, created_at DESC);

CREATE TABLE receipts (
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  tool        TEXT NOT NULL,
  request_id  TEXT NOT NULL,
  args_hash   BLOB NOT NULL,
  status      TEXT NOT NULL CHECK (status IN ('pending', 'done', 'failed')),
  result      TEXT,
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (instance_id, tool, request_id)
);

CREATE TABLE snapshots (
  id          TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  tag         TEXT NOT NULL,         -- qcow2 snapshot tag
  live        INTEGER NOT NULL,
  created_by  TEXT NOT NULL CHECK (created_by IN ('owner', 'agent')),
  created_at  INTEGER NOT NULL
);

CREATE TABLE oauth_clients (
  client_id     TEXT PRIMARY KEY,
  client_name   TEXT NOT NULL,
  redirect_uris TEXT NOT NULL,       -- JSON array
  created_at    INTEGER NOT NULL,
  last_used_at  INTEGER
);

CREATE TABLE oauth_requests (
  id_hash      BLOB PRIMARY KEY,
  client_id    TEXT NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
  redirect_uri TEXT NOT NULL,
  state        TEXT,
  challenge    TEXT NOT NULL,
  scope        TEXT,
  resource     TEXT,
  expires_at   INTEGER NOT NULL
);

CREATE TABLE oauth_codes (
  code_hash    BLOB PRIMARY KEY,
  grant_id     TEXT NOT NULL REFERENCES grants(id) ON DELETE CASCADE,
  redirect_uri TEXT NOT NULL,
  challenge    TEXT NOT NULL,
  expires_at   INTEGER NOT NULL,
  used         INTEGER NOT NULL DEFAULT 0
);

-- One grant per approved assistant connection; tokens hang off it.
CREATE TABLE grants (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  client_id   TEXT REFERENCES oauth_clients(client_id) ON DELETE CASCADE, -- NULL for personal tokens
  name        TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE tokens (
  hash        BLOB PRIMARY KEY,
  grant_id    TEXT NOT NULL REFERENCES grants(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('access', 'refresh', 'personal')),
  prefix      TEXT NOT NULL,
  expires_at  INTEGER,
  used        INTEGER NOT NULL DEFAULT 0, -- refresh tokens are single use
  created_at  INTEGER NOT NULL
);
CREATE INDEX tokens_grant ON tokens(grant_id);
