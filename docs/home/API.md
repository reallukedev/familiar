# Familiar: portal API

JSON over HTTPS. The portal authenticates with a session cookie. Every request that
changes something (`POST`, `PUT`, `PATCH`, `DELETE`) must send `X-Dot-CSRF: <csrf>` from
`GET /api/session` and come from the portal's own origin. Errors are:

```ts
type ApiError = { error: { code: string; message: string; field?: string; retry_after_s?: number } };
```

`code` values: `unauthenticated`, `forbidden`, `not_found`, `invalid`, `conflict`,
`rate_limited`, `csrf`, `totp_required`, `budget`, `unavailable`, `internal`,
`reauth_required`, `totp_setup_required`, `locked` (HTTP 423: an encrypted Mac needs its
owner's key, see "Encryption") and `recovery_key_required` (HTTP 409, reset links).
`message` is a full sentence safe to show as-is.

All times are RFC 3339 strings in UTC. IDs are opaque strings.

## Types

```ts
type Role = 'admin' | 'member';
type User = { id: string; username: string; display_name: string; role: Role; totp_enabled: boolean; created_at: string };

type SessionInfo = {
  user: User | null;
  csrf: string | null;
  setup_required: boolean;     // no users exist yet
  require_totp: boolean;       // admin policy: users must enroll TOTP
  version: string;
  public_url: string;
  locked: boolean;             // signed in, but the data key isn't in server memory (a restart)
  recovery_key_needed: boolean; // no recovery key yet, or not confirmed as saved
};

type InstanceState = 'stopped' | 'starting' | 'booting' | 'ready' | 'degraded'
  | 'sleeping' | 'hibernated' | 'stopping' | 'error' | 'encrypting';
type Driver = 'qemu' | 'remote';

type GuestInfo = {
  os_version: string; os_build: string; hostname: string; user: string; uptime_s: number;
  displays: { index: number; width: number; height: number; scale: number; main: boolean }[];
  permissions: { accessibility: boolean | null; screen_recording: boolean | null; full_disk_access: boolean | null };
  disk_free_bytes: number; frontmost_app: string | null; default_browser: string | null;
  capabilities: string[];
};

type Instance = {
  id: string;
  name: string;
  driver: Driver;
  state: InstanceState;
  state_detail: string | null;   // human sentence, e.g. "Waiting for macOS to finish starting"
  paused: boolean;               // owner's kill switch: every tool call is refused
  cpus: number;
  memory_mb: number;
  disk_gb: number;
  sleep_after_minutes: number;   // 0 = never
  hibernate_after_minutes: number; // 0 = never
  snapshot_keep_days: number;    // unkept snapshots older than this are deleted; 0 = forever (default 7)
  snapshot_max_gb: number;       // most space this Mac's snapshots may use; 0 = no limit (default)
  remote_host: string | null;    // driver = remote
  created_at: string;
  last_active_at: string | null;
  booted_at: string | null;
  guest: GuestInfo | null;       // null until the agent has connected once
  mcp_url: string;               // e.g. https://home.example.com/mcp
  counts: { calls_24h: number; errors_24h: number; pending_approvals: number; pending_review: number };
  encrypted: boolean;            // disks, snapshots and saved memory use the owner's key
  locked: boolean;               // encrypted and the owner's key isn't unlocked: can't start,
                                 // wake from hibernation, restart, hibernate or snapshot
  can_encrypt: boolean;          // a Mac on this server from before encryption (never the image builder)
  encrypt_progress: number | null; // 0 to 1 while state is 'encrypting'
};

type PermissionMode = 'allow' | 'ask' | 'deny';
type ToolRisk = 'read' | 'write' | 'send' | 'system';
type PermissionGroup = {
  id: string;                    // 'observe' | 'control' | 'files_read' | 'files_write' | 'shell' | 'clipboard'
                                 // | 'apps_manage' | 'snapshots' | 'outgoing' | 'pack:<pack id>' | 'mcp:<server id>'
  label: string;
  description: string;
  mode: PermissionMode;
  default_mode: PermissionMode;  // 'deny' for app servers: off until turned on
  tools: { name: string; title: string; risk: ToolRisk }[];
  server?: AppServer;            // only on 'mcp:' groups
};

// An MCP server an app on the Mac provides (docs/home/TOOLS.md "App servers").
type AppServer = {
  app: string | null; bundle_id: string | null;
  transport: 'stdio' | 'http';
  detail: string;                // the command (secrets shown as •••) or loopback address
  source: string;                // 'app', or the assistant whose settings list it: 'Claude', 'Claude Code', 'Cursor', 'VS Code'
  tools_error: string | null;    // why listing its tools failed last time
  listed_at: string | null;
};

type Approval = {
  id: string; instance_id: string; tool: string; title: string;
  summary: string;               // e.g. "Send email to sam@example.com: “Lunch Friday”"
  client: string | null; created_at: string; expires_at: string;
  status: 'pending' | 'approved' | 'denied' | 'expired';
};

type Activity = {
  id: string; at: string; tool: string; title: string;
  outcome: 'ok' | 'error' | 'denied' | 'pending';
  duration_ms: number; detail: string | null; error_code: string | null; client: string | null;
};

type AppEntry = {
  id: string;                    // pack id or bundle id
  name: string;
  bundle_id: string | null;
  installed: boolean;
  running: boolean;
  version: string | null;
  scriptable: boolean;
  enabled: boolean;              // its tools are offered to the assistant
  kind: 'builtin' | 'scriptable' | 'custom' | 'ui_only';
  tools: { name: string; title: string; risk: ToolRisk }[];
  icon: string | null;           // /api/instances/:id/apps/:app/icon when available
};

type Snapshot = { id: string; name: string; created_at: string; live: boolean;
  size_bytes: number | null;     // memory saved with a live snapshot; disk-only snapshots share blocks and are 0
  created_by: 'owner' | 'agent';
  kept: boolean;                 // never deleted automatically
  expires_at: string | null };   // when retention deletes it; null when kept or the Mac keeps snapshots forever
type SnapshotUsage = { used_bytes: number; kept_bytes: number;
  limit_bytes: number | null;    // the Mac's snapshot_max_gb in bytes; null for no limit
  kept_over_limit: boolean };    // kept snapshots alone use more than the limit, so nothing is deleted

type Grant = { id: string; client_name: string; client_id: string; created_at: string; last_used_at: string | null };
type AccessToken = { id: string; name: string; prefix: string; created_at: string; expires_at: string | null; last_used_at: string | null };

type HostOverview = {
  hostname: string; version: string; uptime_s: number;
  cpu_count: number; load_1m: number;
  memory_total_mb: number; memory_available_mb: number;
  disk_total_bytes: number; disk_free_bytes: number;
  budget: { memory_mb: number; max_running: number; used_memory_mb: number; running: number };
  checks: { id: string; label: string; ok: boolean; detail: string }[];
  users: number; instances: number;
};
type AdminUser = User & { disabled: boolean; max_instances: number; instance_count: number; last_seen_at: string | null };
type AdminInstance = { id: string; name: string; owner: string; state: InstanceState; memory_mb: number; cpus: number; last_active_at: string | null };
type ImageStatus = {
  base: { present: boolean; size_bytes: number | null; created_at: string | null; macos: string | null };
  firmware: { present: boolean; detail: string };       // OpenCore + OVMF from OSX-KVM
  versions: { id: string; label: string }[];           // e.g. { id: 'sequoia', label: 'macOS Sequoia 15' }
  builder: null | { instance_id: string; macos: string;
    phase: 'downloading' | 'installing' | 'ready_to_promote' | 'promoting' | 'failed';
    progress: number | null; detail: string };
};
type Invite = { id: string; role: Role; created_at: string; expires_at: string; used: boolean };
```

## Session and sign-in

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/session` | | `SessionInfo` |
| POST | `/api/setup` | `{ token, username, display_name, password }` | `SessionInfo & { recovery_key }` (signed in) |
| POST | `/api/login` | `{ username, password }` | `SessionInfo & { recovery_key? }`, or `401 totp_required` with `{ error, challenge }` |
| POST | `/api/login/totp` | `{ challenge, code }` | `SessionInfo & { recovery_key? }` |
| POST | `/api/logout` | | `{}` |
| GET | `/api/invites/:code` | | `{ role, expires_at, reset_username, needs_recovery_key, encrypted_macs }` or `404` |
| POST | `/api/invites/:code/accept` | `{ username, display_name, password }`; reset links: `{ password, recovery_key?, discard_encrypted_macs? }` | `SessionInfo & { recovery_key? }`, or `401 totp_required` with `{ error, challenge }` |

`/api/setup`, `/api/login*` and invite acceptance don't need the CSRF header (there is
no session yet) but still need a same-origin `Origin`.

Passwords: 10 to 256 characters. Usernames: 2 to 32 of `a-z 0-9 . _ -`.

A `recovery_key` (8 groups of 4 base32 characters, `ABCD-EFGH-…`) is in a sign-in
answer only when one was just made: at setup, when joining, at the first sign-in of an
account from before encryption, and after a reset that started over. It is never shown
again; the portal shows it in a sheet and the person confirms they saved it.

Signing in with TOTP unwraps the data key at the password step but keeps it aside until
the code is right; finishing that same challenge (also after a reset link) unlocks.

## Account

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/account` | | `{ user: User, sessions: { id, current, user_agent, ip, created_at, last_seen_at }[] }` |
| PATCH | `/api/account` | `{ display_name }` | `User` |
| POST | `/api/account/password` | `{ current, password }` | `{ recovery_key? }` (other sessions are signed out; the data key is rewrapped, not changed) |
| POST | `/api/account/totp` | | `{ secret, otpauth_url }` (not active until confirmed) |
| POST | `/api/account/totp/confirm` | `{ code }` | `User` |
| DELETE | `/api/account/totp` | `{ password }` | `User` |
| DELETE | `/api/account/sessions/:id` | | `{}` |
| POST | `/api/account/confirm` | `{ password, code? }` | `{ until, recovery_key? }`: unlocks sensitive actions for 10 minutes, and unlocks the person's encrypted Macs if a restart locked them |
| POST | `/api/account/recovery-key` | | `{ recovery_key }`: a new recovery key, shown once; the old one stops working (needs re-auth; `423 locked` if not unlocked) |
| POST | `/api/account/recovery-key/saved` | | `{}`: the person saved it (`recovery_key_needed` turns false) |
| GET | `/api/account/security` | | `SecurityEvent[]` (last 100) |

Sensitive actions (creating tokens, deleting a Mac, disabling TOTP, changing password,
admin user changes) answer `403 reauth_required` unless the session confirmed the
password (and TOTP code when enrolled) in the last 10 minutes. The portal then asks for
the password and retries.

```ts
type SecurityEvent = { id: string; at: string; kind: 'sign_in' | 'sign_in_failed' | 'sign_out' | 'password_changed'
  | 'totp_enabled' | 'totp_changed' | 'totp_disabled' | 'token_created' | 'token_revoked' | 'grant_created' | 'grant_revoked'
  | 'session_revoked' | 'locked_out' | 'recovery_key_created' | 'recovery_key_used' | 'keys_reset';
  ip: string | null; user_agent: string | null; detail: string | null };
```

## Encryption

Each person's Macs on this server are encrypted at rest with a key only their password
or recovery key can open (docs/home/SECURITY.md "Encryption at rest"). The server holds
the opened key in memory from sign-in until it restarts. After a restart:

- `SessionInfo.locked` and `Instance.locked` are true. Running Macs keep running, but a
  locked Mac can't start, wake from hibernation, restart, hibernate or take or restore
  snapshots while off: those answer `423 locked`. Macs that were meant to be running
  start again on their own once the owner unlocks; hibernated ones wake on demand.
- `POST /api/account/confirm` (password and code) unlocks, as does signing in again.
- The assistant's tools answer `MAC_LOCKED` with a sentence asking the person to sign in.

Reset links (from an admin or `dot-home reset-password`) can't open the key with the new
password. When the person has encrypted Macs, `needs_recovery_key` is true and accepting
needs either `recovery_key` (wrong: `400 invalid` with `field: 'recovery_key'`) or
`discard_encrypted_macs: true`, which makes a new key and deletes those Macs, whose data
can't be read any more. Neither: `409 recovery_key_required`.

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| POST | `/api/instances/:id/encrypt` | | `Instance` (`state: 'encrypting'`; progress arrives as `state` events). Only for `can_encrypt` Macs that are shut down (`409 conflict` otherwise, `423 locked` without the key). Converting drops the Mac's snapshots. |

## Instances

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances` | | `Instance[]` |
| POST | `/api/instances` | `{ name, driver, cpus?, memory_mb?, disk_gb?, remote_host? }` | `{ instance: Instance, pairing?: string }` (`pairing` only for `remote`, shown once) |
| GET | `/api/instances/:id` | | `Instance` |
| PATCH | `/api/instances/:id` | `{ name?, paused?, cpus?, memory_mb?, sleep_after_minutes?, hibernate_after_minutes?, snapshot_keep_days?, snapshot_max_gb? }` | `Instance` (cpu/memory apply on next start; `snapshot_keep_days` 1 to 3650 or 0 for forever, `snapshot_max_gb` 0 to 100000 with 0 for no limit, and a change runs the snapshot sweep right away) |
| DELETE | `/api/instances/:id` | `{ confirm: <instance name> }` | `{}` |
| POST | `/api/instances/:id/power` | `{ action: 'start' \| 'stop' \| 'restart' \| 'force_stop' \| 'sleep' \| 'wake' }` | `Instance` (`423 locked` to start, wake or restart a locked Mac) |
| GET | `/api/instances/:id/thumbnail` | | `image/jpeg`, 640 px wide, cached up to 5 s; `204` when not running |
| GET | `/api/instances/:id/screen` | WebSocket, subprotocol `binary` | raw RFB (VNC) stream for noVNC; `qemu` only |
| GET | `/api/instances/:id/events` | Server-Sent Events | see below |

### Events (SSE)

Each event is `event: <type>` with JSON `data`:

- `instance`: `Instance` (any state or field change)
- `activity`: `Activity` (new or updated entry)
- `approval`: `Approval` (new or resolved)
- `review`: `ReviewItem` (new, decided or delivered), or `{ accepted: number }` after Accept All
- `image`: `ImageStatus` (admins only, while a builder exists)
- `ping`: `{}` every 20 s

## Permissions and approvals

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/permissions` | | `PermissionGroup[]` |
| PUT | `/api/instances/:id/permissions/:group` | `{ mode }` | `PermissionGroup` |
| GET | `/api/instances/:id/approvals` | | `Approval[]` (pending first, then the last 50) |
| POST | `/api/instances/:id/approvals/:approval` | `{ decision: 'approve' \| 'deny' }` | `Approval` |
| POST | `/api/instances/:id/app-servers/refresh` | | `PermissionGroup[]` (all of them), or 503 `unavailable` when the Mac isn't running or its agent is too old |
| GET | `/api/instances/:id/permissions/mode` | | `{ mode: ApprovalMode }` |
| PUT | `/api/instances/:id/permissions/mode` | `{ mode: 'ask' \| 'changes' \| 'all' }` | `{ mode: ApprovalMode, groups: PermissionGroup[] }` |

### Approval modes

```ts
type ApprovalMode = 'ask' | 'changes' | 'all' | 'custom';
```

One choice that sets every group of a Mac in one request:

- `ask` (Ask for Everything): every group is `ask`.
- `changes` (Ask Before Changes, new Macs): groups whose tools all only read (`observe`,
  `files_read`, read-only app packs) are `allow`; every other group is `ask`. `clipboard`
  both reads and replaces, so it asks.
- `all` (Allow Everything): every group is `allow`.

A group set to `deny` stays `deny` in every mode, and app servers (`mcp:` groups) are
left as the owner set them. The mode is stored on the Mac, so groups
that appear later (an app enabled afterwards) follow it until set by hand. `GET` reports
`custom` when the groups no longer match any mode. Macs made before modes existed keep
each group's built-in default until a mode is chosen.

## Review

Every tool call that changed something (any tool whose risk isn't `read`), ran without
asking first and succeeded is kept for the owner to look over, typically at the end of a
session. Accepting files it away. Declining, with an optional note, goes back to the
assistant connection (OAuth grant or access token) that made the change: the next tool
result that connection receives starts with a text block, and has an `owner_feedback`
field in `structuredContent` when that's an object, reading
`The owner declined your earlier change: <title> (<summary>). Their note: <note>`. Each
decline is delivered once. Assistants can also call the MCP tool `get_owner_feedback`,
which returns `{ declined: [{ tool, title, summary, note, made_at, declined_at }] }` and
marks them delivered.

```ts
type ReviewItem = {
  id: string; instance_id: string;
  activity_id: string | null;    // the Activity row of the call
  tool: string; title: string; summary: string;
  client: string | null;         // assistant name
  session: string | null;        // MCP session id; group by it
  connection_id: string | null;  // the grant the decline goes back to
  created_at: string;
  status: 'pending' | 'accepted' | 'declined';
  note: string | null;           // with a decline, up to 1000 characters
  decided_at: string | null;
  delivered_at: string | null;   // when a decline reached the assistant
};
```

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/review?status=pending\|done` | | `ReviewItem[]` (pending: newest first, up to 500; done: last 200 decided) |
| POST | `/api/instances/:id/review/:item` | `{ decision: 'accept' \| 'decline', note?: string }` | `ReviewItem`; 409 if already decided |
| POST | `/api/instances/:id/review/accept-all` | `{ session?: string, ids?: string[] }` | `{ accepted: number }` (all pending when both are absent) |

## Activity

| Method | Path | Query | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/activity` | `before?=<id>&limit?=50&outcome?=error` | `{ items: Activity[], next: string \| null }` |

## Apps

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/apps` | | `AppEntry[]` (needs the Mac running for installed state; otherwise the last known list) |
| PUT | `/api/instances/:id/apps/:app` | `{ enabled }` | `AppEntry` |
| POST | `/api/instances/:id/apps/install` | `{ source: { type:'url', url, sha256? } \| { type:'app_store', id } \| { type:'brew', cask } }` | `AppEntry` |
| POST | `/api/instances/:id/apps/custom` | `{ manifest: string }` (TOML) | `AppEntry` or `400 invalid` with `field` |
| DELETE | `/api/instances/:id/apps/custom/:app` | | `{}` |

## Snapshots

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/snapshots` | | `Snapshot[]` |
| POST | `/api/instances/:id/snapshots` | `{ name }` | `Snapshot` |
| POST | `/api/instances/:id/snapshots/:sid/restore` | | `Instance` |
| PUT | `/api/instances/:id/snapshots/:sid` | `{ kept }` | `Snapshot` |
| DELETE | `/api/instances/:id/snapshots/:sid` | | `{}` |
| GET | `/api/instances/:id/snapshot-usage` | | `SnapshotUsage` |

Snapshots delete themselves. The server sweeps at startup and then hourly, using
the same path as a manual delete (the qcow2 tag, the row and the storage
measurement go together), and logs each deletion:

- **Retention**: unkept snapshots older than the Mac's `snapshot_keep_days` are
  deleted (`0` keeps them forever).
- **Caps**: when a Mac's snapshots use more than its `snapshot_max_gb`, or all
  snapshots on the server use more than `snapshot_total_gb` (see Storage), the
  oldest unkept snapshots are deleted until they fit. Kept snapshots count toward
  the total but are never deleted; if they alone pass a cap, nothing is deleted
  (`SnapshotUsage.kept_over_limit`). Caps also run right after a snapshot is taken,
  never deleting the new one.
- A snapshot being taken or restored is never deleted; restoring one that the
  sweeper is deleting at that moment fails with `503`.
- A snapshot's size is the memory it saved (`vm-state-size`), the same figure
  Storage counts as snapshots; disk-only snapshots share blocks with the disk and
  count as 0.
- The assistant's checkpoints follow the same rules. Besides that, only its newest
  10 unkept checkpoints are kept.

## Connections

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/connections` | | `{ mcp_url, grants: Grant[], tokens: AccessToken[] }` |
| POST | `/api/instances/:id/tokens` | `{ name, expires_in_days? }` | `{ token: AccessToken, secret: string }` (secret shown once) |
| DELETE | `/api/instances/:id/tokens/:tid` | | `{}` |
| DELETE | `/api/instances/:id/grants/:gid` | | `{}` (revokes every token of that grant) |

## Files, clipboard and links

The owner using their own Mac from the portal (the Files page, the screen's
toolbar, drag and drop). These bypass assistant permissions and approvals
because the person is acting themselves, but each upload, download, new
folder, trash and opened link is written to the activity log with the client
`Familiar portal` (clipboard reads and writes are not logged: their content is
private and they happen constantly while syncing). The Mac must be running
(`ready` or `degraded`), else `503 unavailable` naming the Mac. Remote Macs work
the same way through their agent.

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/instances/:id/files?path=` | | `FolderListing` (`~` and `~/x` expand to the Mac user's home; default `~`) |
| GET | `/api/instances/:id/files/download?path=` | | the file's bytes, `Content-Disposition: attachment`, streamed in chunks; files up to 2 GB; folders `400 invalid` |
| POST | `/api/instances/:id/files/upload?path=<folder>&name=<file name>` | raw bytes (`application/octet-stream`) | `FileItem` |
| POST | `/api/instances/:id/files/folder` | `{ path }` | `FileItem` (`409 conflict` when it exists) |
| POST | `/api/instances/:id/files/trash` | `{ path }` | `{}` (moves to the Mac's Trash; the home folder itself is refused) |
| GET | `/api/instances/:id/clipboard` | | `{ text: string \| null }` |
| PUT | `/api/instances/:id/clipboard` | `{ text }` (at most 1 MB) | `{}` |
| POST | `/api/instances/:id/open` | `{ url }` (http or https only) | `{}`: opens in the Mac's default browser |

Uploads: up to 2 GB (`413`), written to a hidden temporary file in the folder
and renamed when complete, so a cancelled upload never leaves a half file. A
name that already exists becomes `name 2.ext`, `name 3.ext`, like Finder. Names
containing `/`, NUL, or equal to `.` or `..` are `400 invalid` (field `name`).

```ts
type FileKind = 'file' | 'dir' | 'symlink' | 'other';
type FileItem = { name: string; path: string; kind: FileKind; size: number; modified: string | null };
type FolderListing = { path: string; parent: string | null; entries: FileItem[]; truncated: boolean };
// entries: folders first, then by name (case-insensitive); hidden files left out; at most 500.
```

## Storage

Everything a person's Macs keep on the server's disk counts against their
storage limit: each Mac's disk overlay (only what it actually wrote, not its
size), its snapshots, and the memory saved while it hibernates. Sizes are the
bytes allocated on disk, measured at most once a minute and cached.

Limits come from server defaults (Admin > Storage) with per-person overrides.
`0` means no limit for `quota_gb` and `max_snapshots`.

- **Creating a Mac** is refused with `409 budget` when its `disk_gb` is over the
  person's `max_disk_gb`, or when they are already at or over their quota.
- **Taking a snapshot** (owner or agent) is refused with `409 budget` when they
  are over quota or the Mac already has `max_snapshots`.
- **Server reserve**: new Macs and snapshots also stop when the server's free
  space is under `reserve_gb` (this replaces `limits.min_free_disk_gb` from the
  config file, which becomes the default for `reserve_gb`).
- Being over quota never stops or deletes anything that exists; the portal
  shows it and points at what to clean up. Only the snapshot caps delete (see
  Snapshots).

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/storage` | | `MyStorage` |
| POST | `/api/instances/:id/storage/discard-memory` | | `Instance`: a `hibernated` Mac becomes `stopped` and its saved memory is deleted (`409 conflict` otherwise) |
| GET | `/api/admin/storage` | | `AdminStorage` |
| PUT | `/api/admin/storage/defaults` | any of `StorageDefaults` | `AdminStorage` (needs re-auth) |
| PUT | `/api/admin/users/:id/storage` | any of `StorageOverrides` (`null` clears an override) | `AdminStorage` (needs re-auth) |

Validation: `quota_gb` 0 to 100000; `max_disk_gb` and `default_disk_gb` 64 to 4096,
with `default_disk_gb <= max_disk_gb`; `max_snapshots` 0 to 100; `reserve_gb` 5 to 10000;
`snapshot_total_gb` 0 to 100000.

```ts
type StorageLimits = { quota_gb: number; max_disk_gb: number; max_snapshots: number };
type MacStorage = { instance_id: string; name: string; driver: Driver; state: InstanceState; disk_gb: number;
  used_bytes: number; disk_bytes: number; snapshots_bytes: number; memory_bytes: number; snapshot_count: number };
type MyStorage = { limits: StorageLimits; sources: Record<keyof StorageLimits, 'default' | 'override'>;
  used_bytes: number; over_quota: boolean; macs: MacStorage[]; measured_at: string };
type StorageDefaults = StorageLimits & { default_disk_gb: number; reserve_gb: number;
  snapshot_total_gb: number };   // all snapshots on the server together; past it the oldest unkept go. 0 = no limit
type StorageOverrides = { quota_gb: number | null; max_disk_gb: number | null; max_snapshots: number | null };
type PersonStorage = { user_id: string; username: string; display_name: string; role: Role; used_bytes: number;
  mac_count: number; overrides: StorageOverrides; limits: StorageLimits; over_quota: boolean };
type AdminStorage = { disk_total_bytes: number; disk_free_bytes: number; images_bytes: number; committed_gb: number;
  defaults: StorageDefaults; people: PersonStorage[]; measured_at: string };
```

Server defaults when never set: `quota_gb` 0 (no limit), `max_disk_gb` 512,
`max_snapshots` 0 (no limit), `default_disk_gb` = `vm.default_disk_gb`,
`reserve_gb` = `limits.min_free_disk_gb`, `snapshot_total_gb` 0 (no limit). `committed_gb` is 0 when anyone's
quota is unlimited.

## Preferences

Saved per account (they follow the person to every device). `PUT` merges the
keys it gets; unknown keys are ignored; a bad value is `400 invalid` with its
`field`.

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/account/preferences` | | `Preferences` |
| PUT | `/api/account/preferences` | any of `Preferences` | `Preferences` |

```ts
type Preferences = {
  appearance: 'system' | 'light' | 'dark';          // default 'system'
  start_page: 'home' | 'last_mac';                  // default 'home'
  screen_mode: 'view' | 'control';                  // default 'view'
  screen_quality: 'auto' | 'sharp' | 'balanced' | 'fast'; // default 'auto'
  screen_scale: 'fit' | 'actual';                   // default 'fit'
  clipboard_sync: boolean;                          // default true
  map_ctrl_to_cmd: boolean;                         // default true
  approval_notifications: boolean;                  // default false
  approval_sound: boolean;                          // default false
  undo_seconds: 0 | 3 | 5 | 10;                     // default 3; how long destructive actions wait with Undo (0 = at once)
};
```

## Push notifications

Web Push (RFC 8030) for approvals, so a request reaches the person with no
Familiar tab open. The server signs with a VAPID key (RFC 8292) it makes on
first start (sealed with the master key in `settings`) and encrypts each
payload for the browser (RFC 8291, `aes128gcm`). Subscriptions are per person
and per browser, labelled with the subscribing session's user agent.

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/push/key` | | `{ public_key }`: uncompressed P-256 point, base64url, for `applicationServerKey` |
| POST | `/api/push/subscriptions` | `{ endpoint, keys: { p256dh, auth } }` (`PushSubscription.toJSON()`) | `{}` |
| DELETE | `/api/push/subscriptions` | `{ endpoint }` | `{}` (also for an unknown endpoint) |

`endpoint` must be `https` on a known push service (`fcm.googleapis.com`,
`android.googleapis.com`, `jmt17.google.com`,
`*.push.services.mozilla.com`, `*.push.apple.com`, `*.notify.windows.com`);
anything else, or a key that isn't a P-256 point / 16-byte secret, is
`400 invalid` with `field` `endpoint`, `keys.p256dh` or `keys.auth`. Posting
an endpoint again updates it. Each person keeps at most 20 browsers; the
oldest make room.

When an approval is created for a Mac whose owner has
`approval_notifications` on, every one of their browsers gets one push
(at most once per approval, in the background, `TTL` = the approval's 15
minutes, `Urgency: high`):

```ts
type ApprovalPush = {
  title: string;        // "Allow Claude to send email?"
  body: string;         // the approval's summary
  url: string;          // "/macs/<instance id>"
  approval_id: string;
  instance_id: string;
};
```

A `404` or `410` from the push service deletes that subscription; other
failures are logged.

## OAuth consent

The authorization endpoint redirects the browser to `/connect?request=<id>`.

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/oauth/requests/:id` | | `{ id, client_name, redirect_host, expires_at }` |
| POST | `/api/oauth/requests/:id` | `{ instance_id, approve: boolean }` | `{ redirect: string }` (the portal navigates there) |

## Admin (role `admin`)

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/admin/overview` | | `HostOverview` |
| GET | `/api/admin/users` | | `AdminUser[]` |
| PATCH | `/api/admin/users/:id` | `{ disabled?, role?, max_instances? }` | `AdminUser` |
| DELETE | `/api/admin/users/:id` | `{ confirm: <username> }` | `{}` (stops and deletes their Macs) |
| POST | `/api/admin/users/:id/reset` | | `{ invite_url }` (one-time password reset link) |
| GET | `/api/admin/instances` | | `AdminInstance[]` |
| POST | `/api/admin/instances/:id/stop` | | `{}` |
| GET | `/api/admin/invites` | | `Invite[]` |
| POST | `/api/admin/invites` | `{ role, expires_in_hours? }` | `{ invite: Invite, url: string }` |
| DELETE | `/api/admin/invites/:id` | | `{}` |
| GET | `/api/admin/images` | | `ImageStatus` |
| POST | `/api/admin/images/builder` | `{ macos: string }` (one of `ImageStatus.versions[].id`) | `ImageStatus` |
| POST | `/api/admin/images/promote` | | `ImageStatus` (builder's disk becomes the golden image) |
| DELETE | `/api/admin/images/builder` | | `ImageStatus` |
| PUT | `/api/admin/settings` | `{ require_totp?, memory_budget_mb?, max_running? }` | `HostOverview` |

## Other endpoints

- `/mcp`: MCP streamable HTTP (bearer token).
- `/.well-known/oauth-authorization-server`, `/.well-known/oauth-protected-resource`,
  `/oauth/register`, `/oauth/authorize`, `/oauth/token`, `/oauth/revoke`.
- `/healthz` (process up), `/readyz` (database reachable), `/metrics` (Prometheus text,
  loopback only).

## Updates

Two channels: `stable` (tagged releases) and `nightly` (every push to the branch). The
server updates itself in place (Macs keep running) and rolls back automatically if the
new build doesn't come up. Each Mac's agent (dot-guest) updates over its link.

```ts
type Channel = 'stable' | 'nightly';
type AgentChannel = 'default' | Channel | 'off';   // 'default' follows the server's guest_default_channel

type Release = { version: string; commit: string; published_at: string; notes: string | null; url: string | null };

type UpdateEvent = {
  id: string; at: string;
  target: 'server' | 'agent'; target_name: string;       // "Familiar" or the Mac's name
  from_version: string | null; to_version: string;
  outcome: 'installed' | 'rolled_back' | 'failed'; detail: string | null;
};

type UpdatesStatus = {
  enabled: boolean;                 // false when the build has no update key or repository
  disabled_reason: string | null;   // sentence, e.g. "This build wasn't made by the release pipeline, so it can't verify updates."
  current: { version: string; commit: string | null; channel: Channel | null };
  previous: { version: string } | null;          // available for Roll Back
  settings: {
    channel: Channel | 'off';                     // what the server itself follows; 'off' = never check
    auto_install: boolean;                        // install server updates in the window without asking
    window_start_hour: number;                    // 0-23, server local time; the window is 2 hours
    guest_default_channel: Channel | 'off';       // what Macs on 'default' follow
    guest_auto_install: boolean;                  // agents update themselves when their Mac is idle
  };
  available: (Release & { newer: boolean }) | null; // latest on the server's channel; newer=false when it's the running build or older (channel switch)
  last_checked_at: string | null;
  state: 'idle' | 'checking' | 'downloading' | 'installing' | 'restarting' | 'failed';
  detail: string | null;
  history: UpdateEvent[];           // newest first, last 50
};

// Instance gains:
type InstanceAgent = {
  version: string | null;           // running dot-guest version (null until connected)
  channel: AgentChannel;
  effective_channel: Channel | 'off';
  available: string | null;         // version this Mac would update to, if different
  updating: boolean;
  can_update: boolean;              // the agent supports self-update (older agents need a manual reinstall)
};
// Instance.agent: InstanceAgent
```

| Method | Path | Body | Returns |
| --- | --- | --- | --- |
| GET | `/api/admin/updates` | | `UpdatesStatus` |
| PUT | `/api/admin/updates` | any of `settings` | `UpdatesStatus` (needs re-auth) |
| POST | `/api/admin/updates/check` | | `UpdatesStatus` (checks now) |
| POST | `/api/admin/updates/install` | | `UpdatesStatus` (installs `available`, even if older after a channel switch; needs re-auth). The server restarts within seconds; the portal reconnects on its own. |
| POST | `/api/admin/updates/rollback` | | `UpdatesStatus` (back to `previous`; needs re-auth) |
| PATCH | `/api/instances/:id` | `{ agent_channel: AgentChannel }` | `Instance` |
| POST | `/api/instances/:id/agent/update` | | `Instance` (updates now; the Mac must be running) |

SSE: `GET /api/admin/events` (admins) streams `updates` (`UpdatesStatus`) and `image` (`ImageStatus`) events, plus `ping`.
