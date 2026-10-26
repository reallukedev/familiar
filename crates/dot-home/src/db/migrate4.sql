-- Portal preferences that follow a person to every device (JSON object of known keys).
ALTER TABLE users ADD COLUMN preferences TEXT;
-- Per-person storage overrides; NULL uses the server default from `settings`.
ALTER TABLE users ADD COLUMN storage_quota_gb INTEGER;
ALTER TABLE users ADD COLUMN max_disk_gb INTEGER;
ALTER TABLE users ADD COLUMN max_snapshots INTEGER;
