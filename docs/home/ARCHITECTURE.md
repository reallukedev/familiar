# Familiar: architecture

Familiar runs on a shared Linux home server. Every person who signs in gets their own
private macOS virtual machine (OSX-KVM on QEMU/KVM), and their AI assistant drives that
Mac through MCP: it sees the screen, clicks, types, runs commands, edits files, uses
Safari, and works with any app installed on it. Nothing is a stored web session any
more; the agent uses real apps on a real Mac.

```
ChatGPT / Claude ──OAuth 2.1──▶ /mcp ──┐
                                       │   dot-home (one Rust binary, systemd)
Browser ──cookie + CSRF──▶ /api ───────┤   ├─ accounts, sessions, TOTP, invites (SQLite)
          └─ websocket ──▶ /screen ────┤   ├─ OAuth authorization server + personal tokens
                                       │   ├─ tool registry: built-in Mac tools + app packs
                                       │   ├─ permissions (allow / ask / deny) + approvals
                                       │   ├─ activity log, receipts, live events (SSE)
                                       │   └─ supervisor per instance
                                       │        ├─ QEMU (OSX-KVM) via QMP: boot, sleep,
                                       │        │   hibernate, snapshots, VNC on a unix socket
                                       │        └─ guest link: TCP + mutual HMAC ──▶ dot-guest
                                       ▼
                       macOS VM (one per instance)
                       └─ dot-guest (LaunchAgent in the signed-in user's session)
                          screencapture, Accessibility, CGEvent input via JXA,
                          osascript, zsh, Safari scripting, Shortcuts, files
```

## Pieces

| Crate / folder | What it is |
| --- | --- |
| `crates/dot-wire` | The host/guest protocol: length-prefixed JSON frames, a mutual HMAC handshake, and every operation as a typed enum. |
| `crates/dot-guest` | The agent inside each Mac. Runs as a LaunchAgent in the user's GUI session so it can see and drive the screen. Uses only system tools (no private APIs, no unsafe code). |
| `crates/dot-home` | The server: HTTP, auth, OAuth, MCP, tools, permissions, VM supervision, the web app. |
| `home-web/` | The portal (React + Base UI + Vite), embedded into the `dot-home` binary at build time. |
| `crates/dot-home/packs/` | Built-in app packs: Calendar, Reminders, Notes, Mail, Messages, Contacts, Music, Finder, Photos. Safari has native tools instead. |
| `deploy/home/`, `Dockerfile` | The single-container setup, compose file, systemd unit. Guides: `SETUP.md`, `SECURITY.md`, `TOOLS.md`. |

## Instances

An instance is one person's Mac. It belongs to exactly one user; nobody else, admins
included, can see its screen, activity, files or tokens. Admins see only name, owner,
state and resource use, and can stop or delete an instance to free resources.

Two drivers:

- **`qemu`** (default): an OSX-KVM VM on this server. Its disk is a qcow2 overlay on the
  shared golden image, so a new Mac is ready in seconds and only stores its own changes.
- **`remote`**: a real Mac elsewhere on a private network (Tailscale, LAN) running
  `dot-guest`. Same tools, no VM features (sleep, snapshots). Useful for a Mac mini.

### Lifecycle

```
stopped ─start─▶ starting ─qemu up─▶ booting ─agent handshake─▶ ready ◀─▶ degraded
   ▲                                                          │  ▲
   │                                              idle N min  │  │ any tool call / viewer
   │                                                          ▼  │
   └──────────── stop / crash (backoff restart) ◀─────── sleeping (vCPUs paused)
                                                              │  ▲
                                                 idle longer  ▼  │ wake (restore RAM file)
                                                         hibernated (RAM on disk, no QEMU)
```

- **Wake on demand.** A tool call to a sleeping or hibernated Mac wakes it and waits.
  A stopped Mac is started. If boot takes longer than the call's budget, the tool returns
  `MAC_STARTING` with `retry_after_ms`, and the portal shows progress.
- **QEMU survives server restarts.** VMs are launched with `-daemonize -pidfile` and a
  QMP socket. On startup the supervisor adopts any VM whose QMP socket answers, so
  upgrading `dot-home` never reboots anyone's Mac.
- **Locked after a restart.** Encrypted Macs need their owner's key, which lives only in
  memory from their sign-in (docs/home/SECURITY.md "Encryption at rest"). After a
  restart an adopted VM keeps running, but a hibernated Mac stays hibernated and one that
  was meant to be running stays stopped (status "Locked. Sign in to unlock.") until the
  owner signs in or confirms their password; then the supervisor starts it again.
- **Encrypting.** A Mac from before encryption can be encrypted while stopped: it moves
  to `encrypting` (starts are refused) while `qemu-img convert` rewrites its disk and
  UEFI variables to LUKS, then back to `stopped`.
- **Crash recovery.** If QEMU dies unexpectedly it is restarted with backoff (5 s, 30 s,
  2 min, then held in `error` for the owner). If the guest agent stops answering for
  5 minutes while QEMU runs, the VM gets an ACPI reset (configurable).
- **Resource budget.** The admin sets a RAM budget and max running Macs. Starting a Mac
  past the budget hibernates the least recently used idle Mac first; if none is idle,
  the start is refused with a clear message.

### Snapshots

Live snapshots (QMP `savevm`, includes memory) while running; disk snapshots
(`qemu-img snapshot`) while stopped. The agent can take a checkpoint before risky work
(`mac_checkpoint`); restoring is done from the portal or by the agent with approval.

Encrypted Macs can't use `savevm` (QEMU aborts writing memory into an encrypted qcow2),
so a live snapshot pauses the vCPUs, takes internal snapshots of the disk and UEFI
variables (`blockdev-snapshot-internal-sync`), streams memory into an encrypted
`snapshots/<tag>.mem` and continues. Restoring one while running restarts QEMU from the
reverted disks and that memory file. Hibernation uses the same encrypted memory stream.

## Guest link

The host dials `127.0.0.1:<forwarded port>` (QEMU user-mode network forwards it to
`dot-guest` on port 7466 in the VM). The guest reads its instance id and secret from a
small read-only FAT disk labelled `DOTCONFIG` that the host builds per instance and
attaches as a USB drive, so one golden image serves every instance.

- Mutual HMAC-SHA256 challenge/response; secrets never cross the wire.
- Many calls in flight on one connection, matched by id; cancelling a call kills its
  child processes in the guest.
- Heartbeat every 5 s; reconnect with backoff; `degraded` after 3 missed beats.
- Every call has a deadline; the guest enforces it too.

## Tools

The tool list an assistant sees depends on its instance: built-in Mac tools, plus one
pack per enabled app. See `docs/home/TOOLS.md` for the list.

Every call goes through one pipeline:

1. Token resolves to (user, instance, client). Paused instance: `PAUSED`.
2. Permission for the tool's group: `allow`, `ask` (the call waits up to 60 s for the
   owner to approve in the portal, which shows a live banner; then `APPROVAL_PENDING`
   with an approval id the agent can wait on), or `deny`.
3. Mac is made ready (wake/start).
4. Concurrency: up to 4 calls per Mac; input (clicks, typing, keys) is serialized so two
   actions can never interleave.
5. Deadline, activity log entry (tool, outcome, duration, short non-content detail),
   live event to the portal.
6. Tools that send things (`mail_send`, `messages_send`) take a `request_id`; a repeat
   returns the first result instead of sending twice.

## Apps

Any app installed in the Mac is usable through the generic tools (UI tree, clicks,
typing, screenshots). Enabling an app in the portal adds more:

- **Built-in packs** give typed tools for Apple apps (`calendar_events`,
  `reminders_create`, `notes_search`, `mail_send`, ...), written as JXA/AppleScript that
  receive arguments as JSON through `argv`, never spliced into source.
- **Scriptable apps**: `app_dictionary` shows any app's AppleScript commands and
  objects, and `mac_script` runs them, so every scriptable app is usable without a pack.
- **Custom packs**: owners can paste a pack manifest (TOML) in the portal.
- **App servers**: MCP servers that apps on the Mac run (Xcode, Figma, or ones listed in
  other assistants' settings) are bridged as `mcp_<server>_<tool>` tools, each off until
  the owner turns it on. See `TOOLS.md`.
- **Installing apps**: Homebrew casks by default (the base image includes Homebrew), or an
  https `.dmg`/`.zip`/`.pkg` (optional SHA-256), or the Mac App Store (`mas`).

Safari is the default browser and has its own tools (tabs, readable page text, DOM
actions, JavaScript, window screenshots).

## Accounts and access

- First run prints a one-time setup link; whoever opens it creates the admin account.
- Further people join by invite link. Passwords are Argon2id; TOTP is optional per user
  and can be required by the admin. Login is rate limited and locks out after repeated
  failures.
- Sessions are `HttpOnly`, `SameSite=Lax` cookies (`__Host-` and `Secure` over HTTPS),
  with a CSRF header on every change and an Origin check.
- Assistants connect with OAuth 2.1 (dynamic client registration, PKCE S256, consent
  screen where the owner picks which Mac to grant, rotating refresh tokens with reuse
  detection). Personal access tokens are available for Claude Code and scripts. Tokens
  are stored hashed and bound to one instance.

## Updates

CI publishes a signed `manifest.json` per channel (`stable` from tags, `nightly` from
every push) with the server binary and the macOS agent. The server verifies the
Ed25519 signature against keys compiled into it and each download's SHA-256.

- **Server**: the new binary goes to `updates/dot-home-<version>`; `updates/launch.json`
  points the launcher at it and the server exec's itself into it. QEMU is daemonized, so
  Macs keep running. The new build is on trial until it has served for a minute; two
  failed starts mark it bad and the launcher falls back to the previous binary. A newer
  container image always wins over an older installed update.
- **Agents**: streamed over the guest link (`UpdateOp`), installed side by side under
  `~/Library/Application Support/Dot Guest/versions/`, and run by a stable `supervise`
  launcher that holds the macOS privacy permissions and rolls back a version that doesn't
  come up.

## Storage

`/var/lib/dot-home` (configurable):

```
dot-home.db            SQLite (WAL): users, sessions, instances, tokens, activity, ...
updates/               launch.json, installed server builds, cached agent builds
master.key             32 random bytes (0600); encrypts TOTP and guest secrets
images/base.qcow2      golden macOS image (read-only, shared)
images/OpenCore.qcow2  OVMF_CODE.fd  OVMF_VARS.fd
instances/<id>/        disk.qcow2  vars.qcow2  config.img  qmp.sock  vnc.sock  qemu.pid
                       hibernate.state  qemu.log  snapshots/<tag>.mem (encrypted Macs)
```

Encrypted Macs' `disk.qcow2` and `vars.qcow2` use QEMU's LUKS encryption, and their
`hibernate.state` and `snapshots/*.mem` are AES-256-GCM streams (`FAMMEM1` header). Keys
reach QEMU through short-lived files in a private runtime directory on a tmpfs
(`vm.runtime_dir`; by default `$RUNTIME_DIRECTORY`, then `/dev/shm`):

```
/run/dot-home/keys/    <random>.key while QEMU or qemu-img starts, then deleted
```

## Network isolation

Each VM has its own QEMU user-mode network: Macs cannot see each other or the LAN
directly. User-mode networking lets a guest reach the server's loopback services at
10.0.2.2, so don't run unauthenticated services on the server's loopback, or set
`network.restrict = true` to cut guest-initiated connections entirely (no internet).
Agent ports are bound to 127.0.0.1 and need the per-instance secret.

## Legal note

Apple's macOS licence permits virtualization only on Apple-branded hardware. Running
OSX-KVM on other hardware is the operator's responsibility. The `remote` driver works
with real Macs.
