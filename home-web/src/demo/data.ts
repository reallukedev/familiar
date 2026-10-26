// SYNTHETIC DEMO DATA. Every name, address, token and event here is invented
// for development and tests. None of it describes a real person or machine.

import type {
  Activity, AdminUser, AppEntry, Approval, HostOverview, ImageStatus, Instance, InstanceAgent, InstanceState, Invite, PermissionGroup,
  Release, SecurityEvent, Snapshot, AccessToken, Grant, AccountSession, UpdateEvent, UpdatesStatus,
} from '../lib/types';

/** Deterministic randomness so screenshots are repeatable. */
export function seeded(seed: number) {
  let t = seed >>> 0;
  return () => {
    t += 0x6d2b79f5;
    let r = Math.imul(t ^ (t >>> 15), 1 | t);
    r ^= r + Math.imul(r ^ (r >>> 7), 61 | r);
    return ((r ^ (r >>> 14)) >>> 0) / 4294967296;
  };
}

export const DEMO_PASSWORD = 'demo-password-1';
export const DEMO_TOTP = '123456';
export const DEMO_ORIGIN = 'https://home.example.test';

const ago = (ms: number) => new Date(Date.now() - ms).toISOString();
const ahead = (ms: number) => new Date(Date.now() + ms).toISOString();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

export type DemoUser = AdminUser & { password: string };

export function users(): DemoUser[] {
  return [
    { id: 'u-alex', username: 'alex', display_name: 'Alex Rivera', role: 'admin', totp_enabled: true, created_at: ago(120 * DAY), disabled: false, max_instances: 12, instance_count: 0, last_seen_at: ago(2 * MIN), password: DEMO_PASSWORD },
    { id: 'u-sam', username: 'sam', display_name: 'Sam Okafor', role: 'member', totp_enabled: false, created_at: ago(40 * DAY), disabled: false, max_instances: 2, instance_count: 0, last_seen_at: ago(3 * HOUR), password: DEMO_PASSWORD },
    { id: 'u-jo', username: 'jo.park', display_name: 'Jo Park', role: 'member', totp_enabled: true, created_at: ago(12 * DAY), disabled: false, max_instances: 1, instance_count: 0, last_seen_at: ago(2 * DAY), password: DEMO_PASSWORD },
    { id: 'u-lee', username: 'lee', display_name: 'Lee Moreno', role: 'member', totp_enabled: false, created_at: ago(5 * DAY), disabled: true, max_instances: 1, instance_count: 0, last_seen_at: null, password: DEMO_PASSWORD },
  ];
}

const GUEST_BASE = {
  os_version: 'macOS Sequoia 15.6',
  os_build: '24G84',
  user: 'dot',
  displays: [{ index: 0, width: 1920, height: 1200, scale: 1, main: true }],
  disk_free_bytes: 61_400_000_000,
  default_browser: 'Safari',
  capabilities: ['screen', 'input', 'shell', 'files', 'safari', 'applescript'],
};

export function instance(id: string, name: string, state: InstanceState, extra: Partial<Instance> = {}): Instance {
  const running = state === 'ready' || state === 'degraded';
  return {
    id,
    name,
    driver: 'qemu',
    state,
    state_detail: null,
    paused: false,
    cpus: 4,
    memory_mb: 8192,
    disk_gb: 128,
    sleep_after_minutes: 30,
    hibernate_after_minutes: 240,
    snapshot_keep_days: 7,
    snapshot_max_gb: 0,
    remote_host: null,
    created_at: ago(30 * DAY),
    last_active_at: running ? ago(3 * MIN) : ago(2 * DAY),
    booted_at: running ? ago(5 * HOUR) : null,
    guest: {
      ...GUEST_BASE,
      hostname: `${id}.local`,
      uptime_s: running ? 5 * 3600 + 720 : 0,
      permissions: { accessibility: true, screen_recording: true, full_disk_access: true },
      frontmost_app: running ? 'Safari' : null,
    },
    mcp_url: `${DEMO_ORIGIN}/mcp`,
    counts: { calls_24h: running ? 142 : 0, errors_24h: running ? 3 : 0, pending_approvals: 0 },
    agent: agent(),
    encrypted: true,
    locked: false,
    can_encrypt: false,
    encrypt_progress: null,
    ...extra,
  };
}

const TOOLS: Omit<Activity, 'id' | 'at' | 'duration_ms' | 'client' | 'outcome' | 'error_code'>[] = [
  { tool: 'screen_capture', title: 'Took a screenshot', detail: 'Main display' },
  { tool: 'ui_click', title: 'Clicked “Reply” in Mail', detail: 'Mail' },
  { tool: 'ui_type', title: 'Typed into a text field', detail: 'Notes' },
  { tool: 'calendar_events', title: 'Listed calendar events', detail: 'Work, next 7 days' },
  { tool: 'safari_open', title: 'Opened a page in Safari', detail: 'developer.apple.com' },
  { tool: 'shell_run', title: 'Ran a shell command', detail: 'brew update' },
  { tool: 'files_read', title: 'Read a file', detail: '~/Documents/notes.md' },
  { tool: 'reminders_create', title: 'Created a reminder', detail: 'Groceries' },
  { tool: 'notes_search', title: 'Searched Notes', detail: '“trip”' },
  { tool: 'ui_tree', title: 'Read the window’s controls', detail: 'Finder' },
];
const CLIENTS = ['Claude', 'ChatGPT', 'Claude Code'];

export function activity(seed: number, count: number, errorRate = 0.08): Activity[] {
  const rand = seeded(seed);
  const items: Activity[] = [];
  let at = Date.now() - 40_000;
  for (let i = 0; i < count; i += 1) {
    const base = TOOLS[Math.floor(rand() * TOOLS.length)]!;
    const roll = rand();
    const outcome: Activity['outcome'] = roll < errorRate ? 'error' : roll < errorRate + 0.03 ? 'denied' : 'ok';
    items.push({
      id: String(seed * 100_000 + (count - i)),
      at: new Date(at).toISOString(),
      ...base,
      outcome,
      duration_ms: Math.round(80 + rand() * (base.tool === 'shell_run' ? 9000 : 1400)),
      error_code: outcome === 'error' ? (rand() < 0.5 ? 'PERMISSION_DENIED' : 'TIMEOUT') : null,
      client: CLIENTS[Math.floor(rand() * CLIENTS.length)]!,
    });
    at -= Math.round(20_000 + rand() * (i < 20 ? 6 * MIN : 3 * HOUR));
  }
  return items;
}

export function approval(instanceId: string, n: number, minutesAgo = 0.3): Approval {
  const kinds = [
    { tool: 'mail_send', title: 'Send email', summary: 'Send email to sam@example.com: “Lunch Friday?”', arguments: { to: ['sam@example.com'], subject: 'Lunch Friday?', body: 'Want to grab lunch Friday at noon?', request_id: '7c0f' } },
    { tool: 'mac_shell', title: 'Run a command', summary: 'Run “brew install --cask visual-studio-code” in Terminal', arguments: { command: 'brew install --cask visual-studio-code' } },
    { tool: 'messages_send', title: 'Send a message', summary: 'Send iMessage to Jo Park: “Running ten minutes late”' },
    { tool: 'files_write', title: 'Change a file', summary: 'Replace ~/Documents/Budget 2026.numbers' },
  ];
  const kind = kinds[n % kinds.length]!;
  return {
    id: `apr-${instanceId}-${n}`,
    instance_id: instanceId,
    ...kind,
    client: n % 2 ? 'ChatGPT' : 'Claude',
    created_at: ago(minutesAgo * MIN),
    expires_at: ahead(60_000 - minutesAgo * MIN + 2 * MIN),
    status: 'pending',
  };
}

export function pastApprovals(instanceId: string): Approval[] {
  return [
    { ...approval(instanceId, 1, 90), id: `apr-${instanceId}-past-1`, status: 'approved', expires_at: ago(88 * MIN) },
    { ...approval(instanceId, 2, 300), id: `apr-${instanceId}-past-2`, status: 'denied', expires_at: ago(298 * MIN) },
    { ...approval(instanceId, 3, 1500), id: `apr-${instanceId}-past-3`, status: 'expired', expires_at: ago(1499 * MIN) },
  ];
}

export function permissions(): PermissionGroup[] {
  const g = (id: string, label: string, description: string, mode: PermissionGroup['mode'], default_mode: PermissionGroup['mode'], tools: PermissionGroup['tools']): PermissionGroup => ({ id, label, description, mode, default_mode, tools });
  return [
    g('observe', 'See the screen', 'Screenshots, the controls in each window, and which app is in front.', 'allow', 'allow', [
      { name: 'screen_capture', title: 'Take a screenshot', risk: 'read' },
      { name: 'ui_tree', title: 'Read window controls', risk: 'read' },
      { name: 'apps_frontmost', title: 'Get the frontmost app', risk: 'read' },
    ]),
    g('control', 'Click and type', 'Move the pointer, click, type and press keys, one action at a time.', 'allow', 'allow', [
      { name: 'ui_click', title: 'Click', risk: 'write' },
      { name: 'ui_type', title: 'Type text', risk: 'write' },
      { name: 'ui_key', title: 'Press keys', risk: 'write' },
      { name: 'ui_scroll', title: 'Scroll', risk: 'write' },
    ]),
    g('files_read', 'Read files', 'Open and list files in the Mac’s home folder.', 'allow', 'allow', [
      { name: 'files_read', title: 'Read a file', risk: 'read' },
      { name: 'files_list', title: 'List a folder', risk: 'read' },
    ]),
    g('files_write', 'Change files', 'Create, edit, move and delete files.', 'ask', 'ask', [
      { name: 'files_write', title: 'Write a file', risk: 'write' },
      { name: 'files_move', title: 'Move or rename', risk: 'write' },
      { name: 'files_delete', title: 'Move to Trash', risk: 'write' },
    ]),
    g('shell', 'Run commands', 'Run commands in a zsh shell as the Mac’s user.', 'ask', 'ask', [{ name: 'shell_run', title: 'Run a shell command', risk: 'system' }]),
    g('clipboard', 'Clipboard', 'Read and set what’s copied.', 'allow', 'allow', [
      { name: 'clipboard_get', title: 'Read the clipboard', risk: 'read' },
      { name: 'clipboard_set', title: 'Set the clipboard', risk: 'write' },
    ]),
    g('apps_manage', 'Install and quit apps', 'Install apps, open them and quit them.', 'ask', 'ask', [
      { name: 'apps_install', title: 'Install an app', risk: 'system' },
      { name: 'apps_quit', title: 'Quit an app', risk: 'write' },
    ]),
    g('snapshots', 'Checkpoints', 'Take a snapshot before risky work. Restoring always asks.', 'allow', 'allow', [{ name: 'mac_checkpoint', title: 'Take a checkpoint', risk: 'write' }]),
    g('outgoing', 'Send messages and email', 'Anything that leaves the Mac in your name.', 'ask', 'ask', [
      { name: 'mail_send', title: 'Send email', risk: 'send' },
      { name: 'messages_send', title: 'Send a message', risk: 'send' },
    ]),
    g('pack:calendar', 'Calendar', 'Read and change events in the Calendar app.', 'allow', 'allow', [
      { name: 'calendar_events', title: 'Calendar events', risk: 'read' },
      { name: 'calendar_create', title: 'Create an event', risk: 'write' },
    ]),
    g('pack:mail', 'Mail', 'Read, search and send email in the Mail app.', 'allow', 'allow', [
      { name: 'mail_search', title: 'Search mail', risk: 'read' },
      { name: 'mail_send', title: 'Send email', risk: 'send' },
    ]),
    g('pack:notes', 'Notes', 'Search and edit notes.', 'deny', 'allow', [
      { name: 'notes_search', title: 'Search notes', risk: 'read' },
      { name: 'notes_create', title: 'Create a note', risk: 'write' },
    ]),
  ];
}

export function apps(long = false): AppEntry[] {
  const pack = (id: string, name: string, bundle: string, enabled: boolean, tools: AppEntry['tools'], running = false): AppEntry => ({ id, name, bundle_id: bundle, installed: true, running, version: '15.6', scriptable: true, enabled, kind: 'builtin', tools, icon: null });
  const list: AppEntry[] = [
    pack('calendar', 'Calendar', 'com.apple.iCal', true, [{ name: 'calendar_events', title: 'Calendar events', risk: 'read' }, { name: 'calendar_create', title: 'Create an event', risk: 'write' }]),
    pack('reminders', 'Reminders', 'com.apple.reminders', true, [{ name: 'reminders_list', title: 'List reminders', risk: 'read' }, { name: 'reminders_create', title: 'Create a reminder', risk: 'write' }]),
    pack('notes', 'Notes', 'com.apple.Notes', true, [{ name: 'notes_search', title: 'Search notes', risk: 'read' }, { name: 'notes_create', title: 'Create a note', risk: 'write' }], true),
    pack('mail', 'Mail', 'com.apple.mail', true, [{ name: 'mail_search', title: 'Search mail', risk: 'read' }, { name: 'mail_send', title: 'Send email', risk: 'send' }], true),
    pack('messages', 'Messages', 'com.apple.MobileSMS', false, [{ name: 'messages_send', title: 'Send a message', risk: 'send' }]),
    pack('contacts', 'Contacts', 'com.apple.AddressBook', false, [{ name: 'contacts_search', title: 'Search contacts', risk: 'read' }]),
    pack('music', 'Music', 'com.apple.Music', false, [{ name: 'music_play', title: 'Play music', risk: 'write' }]),
    pack('finder', 'Finder', 'com.apple.finder', true, [{ name: 'finder_reveal', title: 'Reveal in Finder', risk: 'read' }], true),
    pack('safari', 'Safari', 'com.apple.Safari', true, [{ name: 'safari_tabs', title: 'List tabs', risk: 'read' }, { name: 'safari_open', title: 'Open a page', risk: 'write' }, { name: 'safari_js', title: 'Run JavaScript in a tab', risk: 'system' }], true),
    { id: 'com.culturedcode.ThingsMac', name: 'Things 3', bundle_id: 'com.culturedcode.ThingsMac', installed: true, running: false, version: '3.21', scriptable: true, enabled: false, kind: 'scriptable', tools: [{ name: 'things3_dictionary', title: 'Read the dictionary', risk: 'read' }, { name: 'things3_script', title: 'Run a script', risk: 'write' }], icon: null },
    { id: 'com.apple.Numbers', name: 'Numbers', bundle_id: 'com.apple.Numbers', installed: true, running: false, version: '14.4', scriptable: true, enabled: true, kind: 'scriptable', tools: [{ name: 'numbers_dictionary', title: 'Read the dictionary', risk: 'read' }, { name: 'numbers_script', title: 'Run a script', risk: 'write' }], icon: null },
    { id: 'standup', name: 'Standup Notes', bundle_id: 'com.apple.Notes', installed: true, running: false, version: null, scriptable: true, enabled: true, kind: 'custom', tools: [{ name: 'standup_today', title: 'Today’s standup note', risk: 'read' }], icon: null },
    { id: 'com.microsoft.VSCode', name: 'Visual Studio Code', bundle_id: 'com.microsoft.VSCode', installed: true, running: true, version: '1.105', scriptable: false, enabled: false, kind: 'ui_only', tools: [], icon: null },
    { id: 'com.figma.Desktop', name: 'Figma', bundle_id: 'com.figma.Desktop', installed: true, running: false, version: '125.3', scriptable: false, enabled: false, kind: 'ui_only', tools: [], icon: null },
    { id: 'org.mozilla.firefox', name: 'Firefox', bundle_id: 'org.mozilla.firefox', installed: true, running: false, version: '144.0', scriptable: false, enabled: false, kind: 'ui_only', tools: [], icon: null },
  ];
  if (long) {
    list.push({ id: 'com.example.longname', name: 'Extraordinarily Long Application Name For Testing Wrapping Behaviour Pro Max', bundle_id: 'com.example.extraordinarily.long.application.identifier', installed: true, running: false, version: '2026.10.19-beta.7+build.4411', scriptable: true, enabled: false, kind: 'scriptable', tools: [{ name: 'extraordinarily_long_application_name_dictionary', title: 'Read the dictionary', risk: 'read' }], icon: null });
  }
  return list;
}

export function snapshots(id: string): Snapshot[] {
  return [
    // expires_at is filled in by the demo server from the Mac's retention.
    { id: `${id}-snap-1`, name: 'Before installing Xcode', created_at: ago(26 * HOUR), live: true, size_bytes: 9_800_000_000, created_by: 'owner', kept: false, expires_at: null },
    { id: `${id}-snap-2`, name: 'Checkpoint before “clean up Downloads”', created_at: ago(5 * DAY), live: false, size_bytes: 2_100_000_000, created_by: 'agent', kept: false, expires_at: null },
    { id: `${id}-snap-3`, name: 'Fresh setup', created_at: ago(29 * DAY), live: false, size_bytes: 450_000_000, created_by: 'owner', kept: true, expires_at: null },
  ];
}

export function grants(): Grant[] {
  return [
    { id: 'g-claude', client_name: 'Claude', client_id: 'synthetic-claude', created_at: ago(20 * DAY), last_used_at: ago(4 * MIN) },
    { id: 'g-chatgpt', client_name: 'ChatGPT', client_id: 'synthetic-chatgpt', created_at: ago(9 * DAY), last_used_at: ago(2 * DAY) },
  ];
}

export function tokens(): AccessToken[] {
  return [{ id: 't-1', name: 'Claude Code on MacBook', prefix: 'dot_pat_7Kq2', created_at: ago(15 * DAY), expires_at: ahead(75 * DAY), last_used_at: ago(40 * MIN) }];
}

const SAFARI_MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15';
const SAFARI_IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1';
const CHROME_WIN = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36';

export function sessions(currentUa: string): AccountSession[] {
  return [
    { id: 's-current', current: true, user_agent: currentUa, ip: '198.51.100.24', created_at: ago(2 * HOUR), last_seen_at: ago(0) },
    { id: 's-iphone', current: false, user_agent: SAFARI_IPHONE, ip: '203.0.113.9', created_at: ago(3 * DAY), last_seen_at: ago(5 * HOUR) },
    { id: 's-win', current: false, user_agent: CHROME_WIN, ip: '192.0.2.77', created_at: ago(9 * DAY), last_seen_at: ago(6 * DAY) },
  ];
}

export function security(): SecurityEvent[] {
  return [
    { id: 'e1', at: ago(2 * HOUR), kind: 'sign_in', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: null },
    { id: 'e2', at: ago(3 * HOUR), kind: 'sign_in_failed', ip: '192.0.2.200', user_agent: CHROME_WIN, detail: 'Wrong password' },
    { id: 'e3', at: ago(26 * HOUR), kind: 'token_created', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: 'Claude Code on MacBook' },
    { id: 'e4', at: ago(3 * DAY), kind: 'sign_in', ip: '203.0.113.9', user_agent: SAFARI_IPHONE, detail: null },
    { id: 'e5', at: ago(9 * DAY), kind: 'grant_created', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: 'ChatGPT' },
    { id: 'e6', at: ago(10 * DAY), kind: 'sign_in', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: null },
    { id: 'e7', at: ago(30 * DAY), kind: 'totp_enabled', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: null },
    { id: 'e8', at: ago(31 * DAY), kind: 'sign_in', ip: '198.51.100.24', user_agent: SAFARI_MAC, detail: null },
  ];
}

export function invites(): Invite[] {
  return [
    { id: 'inv-1', role: 'member', created_at: ago(5 * HOUR), expires_at: ahead(67 * HOUR), used: false },
    { id: 'inv-2', role: 'member', created_at: ago(12 * DAY), expires_at: ago(9 * DAY), used: true },
  ];
}

export function overview(failing = false): HostOverview {
  return {
    hostname: 'homelab.example.test',
    version: STABLE[2]!.version,
    uptime_s: 19 * 86_400 + 4 * 3600,
    cpu_count: 16,
    load_1m: 5.4,
    memory_total_mb: 65_536,
    memory_available_mb: 21_000,
    disk_total_bytes: 2_000_000_000_000,
    disk_free_bytes: failing ? 61_000_000_000 : 1_120_000_000_000,
    budget: { memory_mb: 49_152, max_running: 4, used_memory_mb: 32_768, running: 3 },
    checks: [
      { id: 'kvm', label: 'Hardware virtualization', ok: true, detail: '/dev/kvm is available.' },
      { id: 'image', label: 'macOS base image', ok: true, detail: 'macOS Sequoia 15.6, built 12 days ago.' },
      { id: 'disk', label: 'Disk space', ok: !failing, detail: failing ? 'Only 61 GB left. Delete old snapshots or add storage; new Macs and snapshots stop below 40 GB.' : 'Plenty of space.' },
      { id: 'tls', label: 'HTTPS certificate', ok: !failing, detail: failing ? 'The certificate expires in 4 days and renewal failed. Check that port 80 reaches Caddy.' : 'Valid for 61 more days.' },
      { id: 'backup', label: 'Database backup', ok: true, detail: 'Last backup 3 hours ago.' },
    ],
    users: 4,
    instances: 9,
  };
}

export function image(phase: NonNullable<ImageStatus['builder']>['phase'] | null): ImageStatus {
  return {
    base: { present: true, size_bytes: 24_600_000_000, created_at: ago(12 * DAY), macos: 'macOS Sequoia 15.6' },
    firmware: { present: true, detail: 'OpenCore and OVMF from OSX-KVM are installed.' },
    versions: [
      { id: 'tahoe', label: 'macOS Tahoe 26' },
      { id: 'sequoia', label: 'macOS Sequoia 15' },
      { id: 'sonoma', label: 'macOS Sonoma 14' },
    ],
    builder: phase
      ? {
          instance_id: 'builder',
          macos: 'tahoe',
          phase,
          progress: phase === 'downloading' ? 0.42 : phase === 'installing' ? 0.64 : phase === 'promoting' ? 0.3 : null,
          detail:
            phase === 'downloading'
              ? 'Downloading the installer from Apple: 6.1 of 14.5 GB.'
              : phase === 'installing'
                ? 'macOS is installing. When the setup assistant appears, open its screen and finish setup.'
                : phase === 'ready_to_promote'
                  ? 'macOS Tahoe 26 is set up. Save it as the base image when you’re ready.'
                  : phase === 'failed'
                    ? 'The installer stopped: the download didn’t match Apple’s checksum. Discard and try again.'
                    : 'Compacting the disk and saving it as the base image.',
        }
      : null,
  };
}

// ---- Updates ----------------------------------------------------------------

const stamp = (iso: string) => iso.slice(0, 10).replace(/-/g, '');

/** Tagged releases, oldest first. */
export const STABLE: Release[] = [
  { version: '2.9.4', commit: '5be1f07a92c4d8e3b6a1f0c9d2e7b4a8c3f6e1d0', published_at: ago(34 * DAY), notes: 'Fixes a crash when a Mac woke while a snapshot was being taken.', url: 'https://releases.example.test/dot-home/2.9.4' },
  { version: '3.0.0', commit: 'b7e2d41c9a6f3e8d2c5b1a0f9e8d7c6b5a4f3e2d', published_at: ago(12 * DAY), notes: '## What’s new\n- Each person gets their own Macs, with approvals on their phone.\n- The macOS image builder walks you through setup.\n', url: 'https://releases.example.test/dot-home/3.0.0' },
  {
    version: '3.1.0',
    commit: 'e19a7c3f5d2b8e4a6c1f0d9e8b7a6c5d4e3f2a1b',
    published_at: ago(2 * DAY),
    notes: '## What’s new\n- Snapshots show how much space each one uses.\n- Approvals reach your phone a few seconds sooner.\n- Agents update themselves when their Mac is idle.\n\n## Fixes\n- A sleeping Mac no longer wakes twice when two requests arrive together.\n- The installer’s screen keeps its keyboard layout after a restart.',
    url: 'https://releases.example.test/dot-home/3.1.0',
  },
];

/** Builds from the branch, oldest first. Versions carry the build date. */
export function nightlies(): Release[] {
  const older = ago(2 * DAY + 3 * HOUR);
  const newer = ago(5 * HOUR);
  return [
    { version: `3.2.0-nightly.${stamp(older)}`, commit: 'c41de09b7a3f6e2d8c5b4a1f0e9d8c7b6a5f4e3d', published_at: older, notes: 'Move snapshot pruning to the supervisor (#412)', url: null },
    { version: `3.2.0-nightly.${stamp(newer)}`, commit: '8f3c2a91e4b7d6c5a3f2e1d0c9b8a7f6e5d4c3b2', published_at: newer, notes: 'Retry agent updates when a Mac wakes (#418)\nShow free space per snapshot (#417)\nFix approvals count after a restart (#415)', url: null },
  ];
}

export function agent(extra: Partial<InstanceAgent> = {}): InstanceAgent {
  return { version: STABLE[2]!.version, channel: 'default', effective_channel: 'stable', available: null, updating: false, can_update: true, ...extra };
}

function history(): UpdateEvent[] {
  return [
    { id: 'ue-5', at: ago(11 * DAY), target: 'agent', target_name: 'Work Mac', from_version: '2.9.4', to_version: '3.0.0', outcome: 'failed', detail: 'The Mac was asleep. Familiar tries again when it wakes.' },
    { id: 'ue-4', at: ago(12 * DAY - 40 * MIN), target: 'agent', target_name: 'Studio Mac', from_version: '2.9.4', to_version: '3.0.0', outcome: 'installed', detail: null },
    { id: 'ue-3', at: ago(12 * DAY), target: 'server', target_name: 'Familiar', from_version: '2.9.4', to_version: '3.0.0', outcome: 'installed', detail: null },
    { id: 'ue-2', at: ago(33 * DAY), target: 'server', target_name: 'Familiar', from_version: '2.9.3', to_version: '2.9.4', outcome: 'installed', detail: null },
    { id: 'ue-1', at: ago(34 * DAY), target: 'server', target_name: 'Familiar', from_version: '2.9.3', to_version: '2.9.4', outcome: 'rolled_back', detail: 'The new version didn’t start within 2 minutes, so Familiar went back to 2.9.3.' },
  ];
}

export type UpdatesKind = 'current' | 'available' | 'nightly' | 'disabled';

/** The server's update status. "current" is up to date on 3.1.0; "available" still runs 3.0.0. */
export function updates(kind: UpdatesKind): UpdatesStatus {
  const [old, middle, latest] = STABLE as [Release, Release, Release];
  const settings: UpdatesStatus['settings'] = { channel: 'stable', auto_install: true, window_start_hour: 3, guest_default_channel: 'stable', guest_auto_install: true };
  const installedLatest: UpdateEvent = { id: 'ue-6', at: ago(2 * DAY - 3 * HOUR), target: 'server', target_name: 'Familiar', from_version: middle.version, to_version: latest.version, outcome: 'installed', detail: null };
  const base: UpdatesStatus = {
    enabled: true,
    disabled_reason: null,
    current: { version: latest.version, commit: latest.commit, channel: 'stable' },
    previous: { version: middle.version },
    settings,
    available: { ...latest, newer: false },
    last_checked_at: ago(26 * MIN),
    state: 'idle',
    detail: null,
    history: [installedLatest, ...history()],
  };
  switch (kind) {
    case 'current':
      return base;
    case 'available':
      return {
        ...base,
        current: { version: middle.version, commit: middle.commit, channel: 'stable' },
        previous: { version: old.version },
        settings: { ...settings, auto_install: false },
        available: { ...latest, newer: true },
        history: history(),
      };
    case 'nightly': {
      const [built, newest] = nightlies() as [Release, Release];
      return {
        ...base,
        current: { version: built.version, commit: built.commit, channel: 'nightly' },
        previous: { version: latest.version },
        settings: { ...settings, channel: 'nightly', guest_default_channel: 'nightly' },
        available: { ...newest, newer: true },
        history: [{ id: 'ue-7', at: built.published_at, target: 'server', target_name: 'Familiar', from_version: latest.version, to_version: built.version, outcome: 'installed', detail: null }, ...base.history],
      };
    }
    case 'disabled':
      return {
        ...base,
        enabled: false,
        disabled_reason: 'This build wasn’t made by the release pipeline, so it can’t verify updates.',
        current: { version: '3.1.0-dev', commit: null, channel: null },
        previous: null,
        available: null,
        last_checked_at: null,
        history: [],
      };
  }
}
