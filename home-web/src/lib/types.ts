// Types from docs/home/API.md, kept verbatim so the portal and the server agree.
// Additions the contract implies but doesn't name are at the bottom.

export type ApiErrorBody = { error: { code: string; message: string; field?: string; retry_after_s?: number } };

export type Role = 'admin' | 'member';
export type User = { id: string; username: string; display_name: string; role: Role; totp_enabled: boolean; created_at: string };

export type SessionInfo = {
  user: User | null;
  csrf: string | null;
  setup_required: boolean;     // no users exist yet
  require_totp: boolean;       // admin policy: users must enroll TOTP
  version: string;
  public_url: string;
  /** Familiar as this server's Macs reach it (QEMU's internal address). */
  vm_url: string;
  /** This person's data key isn't in the server's memory (it restarted): encrypted Macs can't start until they sign in again. Optional for older servers. */
  locked?: boolean;
  /** They haven't confirmed saving a recovery key yet (or don't have one). */
  recovery_key_needed?: boolean;
};

/** Sign-in, setup and password answers carry a new recovery key once, when one was just made. */
export type WithRecoveryKey = { recovery_key?: string };

export type InstanceState = 'stopped' | 'starting' | 'booting' | 'ready' | 'degraded'
  | 'sleeping' | 'hibernated' | 'stopping' | 'error'
  /** Converting an existing Mac's disk to an encrypted one; see `encrypt_progress`. */
  | 'encrypting';
export type Driver = 'qemu' | 'remote';

export type GuestInfo = {
  os_version: string; os_build: string; hostname: string; user: string; uptime_s: number;
  displays: { index: number; width: number; height: number; scale: number; main: boolean }[];
  permissions: { accessibility: boolean | null; screen_recording: boolean | null; full_disk_access: boolean | null };
  disk_free_bytes: number; frontmost_app: string | null; default_browser: string | null;
  capabilities: string[];
};

export type Instance = {
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
  /** Unkept snapshots older than this are deleted; 0 = forever. Optional for servers older than the contract (7). */
  snapshot_keep_days?: number;
  /** Most space this Mac's snapshots may use, in GB; 0 = no limit. Optional like snapshot_keep_days (0). */
  snapshot_max_gb?: number;
  remote_host: string | null;    // driver = remote
  created_at: string;
  last_active_at: string | null;
  booted_at: string | null;
  guest: GuestInfo | null;       // null until the agent has connected once
  mcp_url: string;               // e.g. https://home.example.com/mcp
  /** pending_review: changes made without asking that wait in Review (optional for older servers). */
  counts: { calls_24h: number; errors_24h: number; pending_approvals: number; pending_review?: number };
  /** From "Updates". Optional here so data from servers older than the contract still renders. */
  agent?: InstanceAgent;
  /** Its disk, snapshots and saved memory are encrypted with the owner's key. Optional for older servers. */
  encrypted?: boolean;
  /** Encrypted, and the owner's key isn't unlocked: it can't start, wake or take snapshots until they sign in. */
  locked?: boolean;
  /** A Mac on this server that isn't encrypted yet (never the image builder). */
  can_encrypt?: boolean;
  /** 0 to 1 while state is `encrypting`. */
  encrypt_progress?: number | null;
};

export type PermissionMode = 'allow' | 'ask' | 'deny';
export type ToolRisk = 'read' | 'write' | 'send' | 'system';
export type PermissionGroup = {
  id: string;                    // 'observe' | 'control' | 'files_read' | 'files_write' | 'shell' | 'clipboard'
                                 // | 'apps_manage' | 'snapshots' | 'outgoing' | 'pack:<pack id>' | 'mcp:<server id>'
  label: string;
  description: string;
  mode: PermissionMode;
  default_mode: PermissionMode;
  tools: { name: string; title: string; risk: ToolRisk }[];
  /** Only on `mcp:` groups: an MCP server an app on the Mac provides. */
  server?: AppServer;
};

/** An MCP server found on the Mac. Off until the owner turns it on; its tools are listed only then. */
export type AppServer = {
  app: string | null;
  bundle_id: string | null;
  transport: 'stdio' | 'http';
  /** The command or loopback address, so the owner recognizes it. */
  detail: string;
  /** `app` when it ships with an app, otherwise the assistant whose settings list it. */
  source: string;
  /** Why listing its tools failed last time. */
  tools_error: string | null;
  /** It was on, but its command or settings changed since, so it's off until turned on again. */
  changed: boolean;
  listed_at: string | null;
};

/** One choice that sets every permission group; `custom` when they match none. */
export type ApprovalMode = 'ask' | 'changes' | 'all' | 'custom';

/** A change the assistant made without asking, waiting for the owner to look over. */
export type ReviewItem = {
  id: string; instance_id: string; activity_id: string | null;
  tool: string; title: string; summary: string;
  client: string | null; session: string | null; connection_id: string | null;
  created_at: string;
  status: 'pending' | 'accepted' | 'declined';
  note: string | null; decided_at: string | null; delivered_at: string | null;
};

export type Approval = {
  id: string; instance_id: string; tool: string; title: string;
  summary: string;               // e.g. "Send email to sam@example.com: “Lunch Friday”"
  client: string | null; created_at: string; expires_at: string;
  status: 'pending' | 'approved' | 'denied' | 'expired';
  /** Every argument of the call, so the owner sees exactly what they approve. */
  arguments?: Record<string, unknown>;
};

export type Activity = {
  id: string; at: string; tool: string; title: string;
  outcome: 'ok' | 'error' | 'denied' | 'pending';
  duration_ms: number; detail: string | null; error_code: string | null; client: string | null;
};

export type AppEntry = {
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

export type Snapshot = { id: string; name: string; created_at: string; live: boolean; size_bytes: number | null; created_by: 'owner' | 'agent';
  /** Marked to keep: never deleted automatically. */
  kept: boolean;
  /** When retention deletes it; null when kept or the Mac keeps snapshots forever. */
  expires_at: string | null };

/** What one Mac's snapshots use against its own cap. */
export type SnapshotUsage = { used_bytes: number; kept_bytes: number; limit_bytes: number | null; kept_over_limit: boolean };

/** Server-wide snapshot cap, sent and returned with StorageDefaults. 0 = no limit. */
export type SnapshotCap = { snapshot_total_gb: number };

export type Grant = { id: string; client_name: string; client_id: string; created_at: string; last_used_at: string | null };
export type AccessToken = { id: string; name: string; prefix: string; created_at: string; expires_at: string | null; last_used_at: string | null };

export type HostOverview = {
  hostname: string; version: string; uptime_s: number;
  cpu_count: number; load_1m: number;
  memory_total_mb: number; memory_available_mb: number;
  disk_total_bytes: number; disk_free_bytes: number;
  budget: { memory_mb: number; max_running: number; used_memory_mb: number; running: number };
  checks: { id: string; label: string; ok: boolean; detail: string }[];
  users: number; instances: number;
};
export type AdminUser = User & { disabled: boolean; max_instances: number; instance_count: number; last_seen_at: string | null };
export type AdminInstance = { id: string; name: string; owner: string; state: InstanceState; memory_mb: number; cpus: number; last_active_at: string | null };
export type ImageStatus = {
  base: { present: boolean; size_bytes: number | null; created_at: string | null; macos: string | null };
  firmware: { present: boolean; detail: string };       // OpenCore + OVMF from OSX-KVM
  versions: { id: string; label: string }[];           // e.g. { id: 'sequoia', label: 'macOS Sequoia 15' }
  builder: null | { instance_id: string; macos: string;
    phase: 'downloading' | 'installing' | 'ready_to_promote' | 'promoting' | 'failed';
    progress: number | null; detail: string };
};
export type Invite = { id: string; role: Role; created_at: string; expires_at: string; used: boolean };

// Updates
export type Channel = 'stable' | 'nightly';
export type AgentChannel = 'default' | Channel | 'off';   // 'default' follows the server's guest_default_channel

export type Release = { version: string; commit: string; published_at: string; notes: string | null; url: string | null };

export type UpdateEvent = {
  id: string; at: string;
  target: 'server' | 'agent'; target_name: string;       // "Familiar" or the Mac's name
  from_version: string | null; to_version: string;
  outcome: 'installed' | 'rolled_back' | 'failed'; detail: string | null;
};

export type UpdatesStatus = {
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

export type InstanceAgent = {
  version: string | null;           // running dot-guest version (null until connected)
  channel: AgentChannel;
  effective_channel: Channel | 'off';
  available: string | null;         // version this Mac would update to, if different
  updating: boolean;
  can_update: boolean;              // the agent supports self-update (older agents need a manual reinstall)
};

export type SecurityEvent = { id: string; at: string; kind: 'sign_in' | 'sign_in_failed' | 'sign_out' | 'password_changed'
  | 'totp_enabled' | 'totp_changed' | 'totp_disabled' | 'token_created' | 'token_revoked' | 'grant_created' | 'grant_revoked'
  | 'session_revoked' | 'locked_out' | 'recovery_key_created' | 'recovery_key_used' | 'keys_reset'; ip: string | null; user_agent: string | null; detail: string | null };

// ---------------------------------------------------------------------------
// Shapes the API tables return inline, named here for reuse.

export type ErrorCode =
  | 'unauthenticated' | 'forbidden' | 'not_found' | 'invalid' | 'conflict' | 'rate_limited' | 'csrf'
  | 'totp_required' | 'budget' | 'unavailable' | 'internal'
  // Documented in "Account": sensitive actions answer 403 reauth_required.
  | 'reauth_required'
  // Server: every call but /api/account*, /api/session and /api/logout while the
  // account still has to enroll in two-factor.
  | 'totp_setup_required'
  // 423: the Mac is encrypted and the owner's key isn't unlocked.
  | 'locked'
  // 409 from a reset link: the account's encrypted Macs need the recovery key.
  | 'recovery_key_required';

export type AccountSession = { id: string; current: boolean; user_agent: string | null; ip: string | null; created_at: string; last_seen_at: string };
export type AccountInfo = { user: User; sessions: AccountSession[] };
export type InviteInfo = {
  role: Role;
  expires_at: string;
  /** A password-reset link names the account it resets. Join links leave it out. */
  reset_username?: string | null;
  /** Older name for reset_username (the demo backend used it). */
  username?: string;
  /** Reset links: the account has encrypted Macs, so the recovery key is needed to keep them. */
  needs_recovery_key?: boolean;
  /** How many encrypted Macs the account has. */
  encrypted_macs?: number;
};
export type ActivityPage = { items: Activity[]; next: string | null };
export type Connections = { mcp_url: string; grants: Grant[]; tokens: AccessToken[] };
export type OAuthRequest = { id: string; client_name: string; redirect_host: string; expires_at: string };
export type PowerAction = 'start' | 'stop' | 'restart' | 'force_stop' | 'sleep' | 'wake';
export type InstallSource = { type: 'url'; url: string; sha256?: string } | { type: 'app_store'; id: string } | { type: 'brew'; cask: string };

// ---------------------------------------------------------------------------
// Storage (docs/home/API.md "Storage"). Sizes are bytes actually used on the
// server's disk; Mac disks are thin, so `disk_gb` is a ceiling, not a cost.

/** Where a person's effective limit came from. */
export type LimitSource = 'default' | 'override';

export type StorageLimits = {
  /** Everything a person's Macs may use together, in GB. 0 = no limit. */
  quota_gb: number;
  /** Largest disk one new Mac may have, in GB. */
  max_disk_gb: number;
  /** Snapshots per Mac. 0 = no limit. */
  max_snapshots: number;
};

export type MacStorage = {
  instance_id: string;
  name: string;
  driver: Driver;
  state: InstanceState;
  /** The disk's ceiling: what macOS inside the Mac sees as its disk size. */
  disk_gb: number;
  used_bytes: number;              // disk + snapshots + memory
  disk_bytes: number;              // the Mac's own disk overlay
  snapshots_bytes: number;         // saved snapshots (live ones include memory)
  memory_bytes: number;            // memory saved while hibernated
  snapshot_count: number;
};

export type MyStorage = {
  limits: StorageLimits;
  sources: Record<keyof StorageLimits, LimitSource>;
  used_bytes: number;
  /** used_bytes >= quota: new Macs and snapshots are refused until space is freed. */
  over_quota: boolean;
  macs: MacStorage[];              // remote Macs are listed with zero usage
  measured_at: string;
};

export type StorageDefaults = StorageLimits & {
  /** Disk size a new Mac gets unless its owner picks another. */
  default_disk_gb: number;
  /** New Macs and snapshots stop when the server's free space falls below this, in GB. */
  reserve_gb: number;
};

/** Per-person overrides. null = use the server default. */
export type StorageOverrides = { [K in keyof StorageLimits]: number | null };

export type PersonStorage = {
  user_id: string;
  username: string;
  display_name: string;
  role: Role;
  used_bytes: number;
  mac_count: number;
  overrides: StorageOverrides;
  limits: StorageLimits;           // effective
  over_quota: boolean;
};

export type AdminStorage = {
  disk_total_bytes: number;
  disk_free_bytes: number;
  /** The shared macOS base image and installers. */
  images_bytes: number;
  /** Space promised by quotas: sum of every person's quota (0 when any is unlimited). */
  committed_gb: number;
  defaults: StorageDefaults & Partial<SnapshotCap>;
  people: PersonStorage[];
  measured_at: string;
};

// ---------------------------------------------------------------------------
// Preferences (docs/home/API.md "Preferences"). Saved per account, so they
// follow the person to every device. Unknown keys are ignored by the server.

export type Preferences = {
  appearance: 'system' | 'light' | 'dark';
  /** Where the portal opens: the Macs list, or the last Mac they used. */
  start_page: 'home' | 'last_mac';
  /** How a Mac's screen opens. */
  screen_mode: 'view' | 'control';
  screen_quality: 'auto' | 'sharp' | 'balanced' | 'fast';
  screen_scale: 'fit' | 'actual';
  /** Copy on the Mac, paste here (and the reverse) while in control. */
  clipboard_sync: boolean;
  /** Send ⌘ for Ctrl shortcuts typed on a Windows or Linux keyboard. */
  map_ctrl_to_cmd: boolean;
  /** Browser notifications for approvals (each device also asks permission). */
  approval_notifications: boolean;
  approval_sound: boolean;
  /** How long destructive actions wait, with Undo, before they run. 0 runs at once. */
  undo_seconds: 0 | 3 | 5 | 10;
};

export const DEFAULT_PREFERENCES: Preferences = {
  appearance: 'system',
  start_page: 'home',
  screen_mode: 'view',
  screen_quality: 'auto',
  screen_scale: 'fit',
  clipboard_sync: true,
  map_ctrl_to_cmd: true,
  approval_notifications: false,
  approval_sound: false,
  undo_seconds: 3,
};

// ---------------------------------------------------------------------------
// Files (docs/home/API.md "Files"): the owner's own file access, not the
// assistant's. Works while the Mac is running.

export type FileKind = 'file' | 'dir' | 'symlink' | 'other';
export type FileItem = { name: string; path: string; kind: FileKind; size: number; modified: string | null };
export type FolderListing = {
  path: string;                    // absolute, resolved (~ expanded)
  parent: string | null;           // null at /
  entries: FileItem[];             // folders first, then by name
  truncated: boolean;              // more than 500 entries
};

// Web Push (docs/home/API.md "Push notifications").

/** `GET /api/push/key`: the server's VAPID public key, base64url. */
export type PushKey = { public_key: string };

/** A browser's push subscription, as `PushSubscription.toJSON()` gives it. */
export type PushSubscriptionBody = { endpoint: string; keys: { p256dh: string; auth: string } };

/** What a push carries; the service worker shows it as a notification. */
export type ApprovalPush = { title: string; body: string; url: string; approval_id: string; instance_id: string };
