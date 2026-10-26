-- Per-person encryption at rest (docs/home/SECURITY.md "Encryption at rest").
-- Each person's data key, stored only wrapped: once with a key derived from their
-- password (Argon2id, with the salt and settings below) and once with their recovery
-- key. Plaintext keys never reach the database.
CREATE TABLE user_keys (
  user_id        TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  kdf_salt       BLOB NOT NULL,
  kdf_memory_kib INTEGER NOT NULL,
  kdf_passes     INTEGER NOT NULL,
  kdf_lanes      INTEGER NOT NULL,
  password_wrap  BLOB NOT NULL,     -- nonce + AES-256-GCM(data key)
  recovery_wrap  BLOB NOT NULL,     -- nonce + AES-256-GCM(data key)
  recovery_saved INTEGER NOT NULL DEFAULT 0, -- the person confirmed they saved it
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);
-- The Mac's disks, firmware variables, snapshots and saved memory are encrypted with a
-- key derived from its owner's data key. Macs made before this stay unencrypted until
-- their owner encrypts them; image builders never are (they become the shared base).
ALTER TABLE instances ADD COLUMN encrypted INTEGER NOT NULL DEFAULT 0;
