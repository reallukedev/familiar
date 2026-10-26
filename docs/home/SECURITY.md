# Familiar security

Familiar is meant to be reachable from the internet. This is what protects it.

## People and sessions

- Argon2id passwords (19 MiB, 2 passes); at least 10 characters; a failed sign-in costs
  the same time whether or not the username exists.
- Two-factor (TOTP) required for everyone by default. Codes can't be replayed; five wrong
  codes end the sign-in attempt.
- Five wrong passwords or codes lock the account for 15 minutes, doubling up to a day
  (counted atomically, so parallel guesses don't help). Sign-in, setup, invite and OAuth
  endpoints are also limited per IP (per /64 for IPv6).
- Password reset links (from an admin or the CLI) replace the password only; the
  authenticator code is still required, so an admin can't sign in as someone. A lost
  phone is handled with `dot-home reset-password <user> --clear-totp` on the server.
  The new password can't open the person's data key, so the reset page also asks for
  their recovery key (see "Encryption at rest").
- Sessions: random 256-bit cookie, stored hashed, `HttpOnly`, `SameSite=Lax`, `Secure`
  with the `__Host-` prefix over HTTPS. 14-day lifetime, 3 days idle. Changing a password
  signs out every other session.
- Every change needs the session's CSRF header and a same-origin `Origin`.
- Creating tokens, deleting a Mac, turning off two-factor and admin changes need the
  password (and code) again within 10 minutes.
- A security log per person: sign-ins (new browsers flagged), failures, lockouts,
  two-factor changes, tokens and connected assistants.
- First-run setup needs a one-time token from the server log, so nobody can claim a
  fresh server first.

## Encryption at rest

Each person's Macs are encrypted with a key that only they can open.

- **Keys.** Every person has a random 256-bit data key. The database stores it only
  wrapped (AES-256-GCM, bound to the account): once with a key derived from their
  password (Argon2id, 64 MiB, 3 passes, its own salt; the settings are stored with the
  wrap), and once with a recovery key (160 random bits, shown as `ABCD-EFGH-…` in 8
  groups). The database holds salts, settings and wrapped blobs, never a key. Each Mac's
  key is derived from the data key with HKDF-SHA256 and the Mac's id.
- **What's encrypted.** A Mac's disk overlay and its UEFI variables are qcow2 files with
  QEMU's LUKS encryption (AES-256-XTS); only the Mac's own blocks are, since the shared
  base image holds nothing personal. Snapshots taken while off are inside those files.
  Saved memory (hibernation, and live snapshots, which QEMU can't write into an encrypted
  qcow2) is streamed from QEMU through a pipe and AES-256-GCM in 1 MiB authenticated
  chunks, with a fresh key per file; QEMU never writes plaintext memory to disk, and a
  changed or truncated file is refused before any of it reaches QEMU.
- **Not encrypted.** `config.img`, the small disk that tells a Mac its id and pairing
  secret, holds no personal data, and that secret is already sealed in the database with
  `master.key` on the same disk, so encrypting it would add nothing. The shared base
  image, the database (accounts, activity log, settings) and remote Macs (they're
  someone's real Mac, outside Familiar's disks) aren't covered either.
- **How QEMU gets the key.** Through `-object secret,file=…` pointing at a 0600 file in a
  private 0700 directory on a tmpfs (`/run/dot-home` under systemd, `/dev/shm` in Docker),
  deleted as soon as QEMU has started or `qemu-img` has finished. It never appears on a
  command line, in `ps`, in `qemu.log` or in Familiar's own log.
- **Unlocking.** Signing in (password and authenticator code) or confirming the password
  unwraps the data key into the server's memory. With two-factor, the key opened by the
  password waits aside until the code is right. It stays in memory (zeroed when dropped)
  until the server restarts. Changing the password rewraps the same key.
- **After a restart** (including installing an update), nobody's key is in memory.
  Running Macs keep running, since QEMU already has their key, but no Mac can start, wake
  from hibernation, restart, hibernate or take snapshots until its owner signs in again
  or confirms their password; the portal shows "Locked" and the assistant gets
  `MAC_LOCKED` asking the person to sign in. Macs that were meant to be running start on
  their own once their owner unlocks. In Docker, stopping the container can't hibernate
  a locked Mac (its memory can't be encrypted), so it cold boots next time.
- **Recovery key.** Shown once at setup, when joining, at the first sign-in of an account
  from before encryption, or from Settings › Account › Recovery Key… (password again;
  the old one stops working). The portal asks the person to confirm they saved it.
- **Reset links.** The new password can't open the old key. With the recovery key, the
  same key is rewrapped and nothing is lost. Without it, the person must agree to erase
  their encrypted Macs: a new key is made and those Macs are deleted, since their data
  can no longer be read (deleting is cleaner than keeping Macs that can never start).
- **Macs from before encryption** keep working unencrypted. Their owner can encrypt one
  from its Settings while it's shut down (`qemu-img convert` to LUKS, then swapped in;
  its snapshots are dropped). The image builder is never encrypted, because it becomes
  everyone's base image.

What this does and doesn't protect against:

- It protects data at rest: a copied or stolen disk or backup of `/var/lib/dot-home`
  (even with `master.key`) doesn't reveal what's inside anyone's encrypted Macs.
- It doesn't protect against someone with root on the running server. While a Mac runs,
  its memory and its key are in QEMU, and every signed-in person's data key is in
  Familiar's memory; root can read both, or wait for the next sign-in. Admins without
  root still see no more than before.
- A lost password plus a lost recovery key means the data is gone. Nobody, the admin
  included, can get it back.

## Assistants

- OAuth 2.1 with PKCE (S256 only), exact redirect URI matching (https, or http on
  loopback), single-use 2-minute codes; a replayed code revokes everything it issued.
- Access tokens last an hour. Refresh tokens rotate on every use; presenting an old one
  revokes the whole connection.
- Every token is bound to one Mac and stored as a SHA-256 hash.
- Personal tokens (for Claude Code) can expire and are revocable on the Connect page.

## Macs

- Each Mac belongs to one person. Other people, admins included, get 404s for it.
  Admins see only name, owner, state and resources.
- Each VM has its own user-mode network. Agent ports are forwarded on 127.77.0.1, an
  address guests can't reach (QEMU maps a guest's 10.0.2.2 to the host's 127.0.0.1 only),
  so one Mac can't even connect to another's agent; the link also needs a per-Mac
  256-bit secret (mutual HMAC challenge, never sent). The VNC screen is a
  private unix socket reachable only through the signed-in owner's WebSocket.
- The owner decides per group: Allow, Ask (live approval in the portal, bound to the
  exact arguments, shown in full, usable once) or Never (tools hidden). Sending email or messages and
  installing apps ask by default. A pause switch stops everything instantly.
- App MCP servers (docs/home/TOOLS.md "App servers") are off until the owner turns each
  one on; approval modes never do. Finding them only reads files and starts nothing.
  The host names a server only by the id the Mac gave it, so it can never make the Mac
  run a command of its choosing. An approval is tied to a fingerprint of the server's
  command, arguments and environment: if any of them change (say, an assistant edits a
  config file), the server is off until the owner turns it on again, and the Mac checks
  the fingerprint on every call. Servers start without reading shell profiles, and
  variables like `NODE_OPTIONS` and `DYLD_*` are dropped. Environment values from other
  assistants' settings (often API keys) never leave the Mac, and credential-looking
  arguments, URL passwords and server error output reach the portal with secrets as
  `•••`. HTTP servers must be on the Mac's own loopback address.
- Known limit: a server finds its program through the user's PATH, read once when the
  agent starts, so replacing a program in a PATH folder still changes what runs.
- Content from the Mac is labelled untrusted for the assistant. The activity log stores
  tool, outcome, timing and a short non-content detail.
- Checkpoints let the owner roll back anything the agent did.

## The server

- Strict CSP (scripts and styles only from itself), HSTS, `X-Frame-Options: DENY`,
  `no-referrer`, COOP/CORP, a restrictive Permissions-Policy. API responses aren't cached.
- Request bodies are capped. The config refuses plain HTTP on a public interface.
- Secrets at rest (two-factor, Mac pairings) are sealed with AES-256-GCM using
  `master.key` (0600), bound to their row. People's Macs are encrypted with their own
  keys instead (see "Encryption at rest").
- The container runs as a non-root user with a read-only root filesystem, no
  capabilities and `no-new-privileges`; only `/dev/kvm` is passed in. The image build
  ignores `secrets/` and `.env` files.

## Known limits

- Terminal and scripts are allowed by default because that's what makes the agent
  capable. A script can also send mail or install software, so "Ask" on Send to other
  people or Install only covers the dedicated tools. Set Terminal and scripts to Ask for
  a strict setup.
- Live screens and event streams recheck the session every 30 seconds, so signing out
  elsewhere ends them within half a minute.
- `/metrics` answers only with `Authorization: Bearer $DOT_HOME_METRICS_TOKEN`.
- Remote Macs must be on private addresses, which also means a member can point one at
  another host on your LAN and see whether it answers on that port.
- QEMU user-mode networking lets a guest reach the server's own loopback services at
  10.0.2.2. Don't run unauthenticated services on the server's loopback, or set
  `vm.restrict_network = true` (no internet inside Macs).
- The remote-Mac link is authenticated but not encrypted, so it's only allowed on private
  addresses; use Tailscale or WireGuard between sites.
- Familiar doesn't do passkeys yet.
