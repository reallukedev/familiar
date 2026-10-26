-- Approval modes and the Review list (docs/home/API.md "Approvals" and "Review").
-- The approval mode a Mac was last set to: 'ask', 'changes' or 'all'. Groups without a
-- stored row follow it; NULL keeps each group's built-in default (Macs made before modes).
ALTER TABLE instances ADD COLUMN approval_mode TEXT;

-- Changes the assistant made without asking, for the owner to look over later.
CREATE TABLE review_items (
  id           TEXT PRIMARY KEY,
  instance_id  TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  activity_id  INTEGER,                 -- the activity row of the call
  tool         TEXT NOT NULL,
  summary      TEXT NOT NULL,
  client       TEXT,                    -- assistant name
  grant_id     TEXT,                    -- the assistant connection a decline goes back to
  session      TEXT,                    -- MCP session, for grouping
  created_at   INTEGER NOT NULL,
  status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'declined')),
  note         TEXT,                    -- the owner's note with a decline
  decided_at   INTEGER,
  delivered_at INTEGER                  -- when a decline reached the assistant
);
CREATE INDEX review_instance ON review_items(instance_id, status, created_at DESC);
CREATE INDEX review_undelivered ON review_items(grant_id, delivered_at) WHERE status = 'declined';
