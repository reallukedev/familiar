-- Snapshot retention and caps (docs/home/API.md "Snapshots").
-- Kept snapshots are never deleted automatically.
ALTER TABLE snapshots ADD COLUMN kept INTEGER NOT NULL DEFAULT 0;
-- Unkept snapshots older than this are deleted; 0 keeps them forever.
ALTER TABLE instances ADD COLUMN snapshot_keep_days INTEGER NOT NULL DEFAULT 7;
-- Most space this Mac's snapshots may use, in GB; 0 = no limit.
ALTER TABLE instances ADD COLUMN snapshot_max_gb INTEGER NOT NULL DEFAULT 0;
