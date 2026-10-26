-- Auto-updates: each Mac's agent channel and a history of installs.
ALTER TABLE instances ADD COLUMN agent_channel TEXT NOT NULL DEFAULT 'default'
  CHECK (agent_channel IN ('default', 'stable', 'nightly', 'off'));
ALTER TABLE instances ADD COLUMN agent_version TEXT;

CREATE TABLE update_log (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  at           INTEGER NOT NULL,
  target       TEXT NOT NULL CHECK (target IN ('server', 'agent')),
  instance_id  TEXT,
  target_name  TEXT NOT NULL,
  from_version TEXT,
  to_version   TEXT NOT NULL,
  outcome      TEXT NOT NULL CHECK (outcome IN ('installed', 'rolled_back', 'failed')),
  detail       TEXT
);
CREATE INDEX update_log_at ON update_log(at DESC);
