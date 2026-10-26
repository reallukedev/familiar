-- Web Push: one row per browser that asked for approval notifications.
CREATE TABLE push_subscriptions (
  endpoint    TEXT PRIMARY KEY,     -- the push service URL; identifies one browser
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  p256dh      TEXT NOT NULL,        -- base64url browser public key (RFC 8291)
  auth        TEXT NOT NULL,        -- base64url authentication secret
  session_id  TEXT,                 -- sessions.public_id that subscribed
  user_agent  TEXT,                 -- device label, as in the sessions list
  created_at  INTEGER NOT NULL
);
CREATE INDEX push_subscriptions_user ON push_subscriptions(user_id);
-- Set once a push went out for an approval, so each approval pushes at most once.
ALTER TABLE approvals ADD COLUMN pushed_at INTEGER;
