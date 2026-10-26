// SYNTHETIC demo endpoints for storage, preferences, files, clipboard and
// links. Development and tests only. File names and contents are invented.
import type {
  AdminStorage, FileItem, FolderListing, Instance, MacStorage, MyStorage, PersonStorage, Preferences, StorageDefaults, StorageLimits, StorageOverrides,
} from '../lib/types';
import { DEFAULT_PREFERENCES } from '../lib/types';
import type { DemoUser } from './data';
import type { DemoState } from './state';

type Result = { status: number; body?: unknown; blob?: Blob };
type Ctx = { state: DemoState; query: URLSearchParams; body: Record<string, unknown> };
type On = (method: string, pattern: string, handler: (ctx: Ctx, ...params: string[]) => Result | Promise<Result>) => void;
type Helpers = {
  json: (body: unknown, status?: number) => Result;
  fail: (status: number, code: string, message: string, extra?: Record<string, unknown>, field?: string) => Result;
  me: (state: DemoState) => DemoUser | null;
  mine: (state: DemoState, id: string) => Instance | null;
  admin: (state: DemoState) => Result | null;
  sensitive: (state: DemoState) => Result | null;
  update: (state: DemoState, instance: Instance, patch: Partial<Instance>) => void;
};

const GB = 1_000_000_000;
const HOME = '/Users/dot';

// `locked` folders answer like macOS privacy protection does without Full Disk Access.
type Node = { kind: 'dir'; children: Map<string, Node>; modified: string; locked?: boolean } | { kind: 'file'; size: number; modified: string; text?: string };

export type ExtrasState = {
  prefs: Record<string, Preferences>;
  defaults: AdminStorage['defaults'];
  overrides: Record<string, StorageOverrides>;
  files: Record<string, Node>;
  clipboard: Record<string, string | null>;
  opened: { instance: string; url: string }[];
};

const day = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();

function dir(entries: Record<string, Node>, age = 3): Node {
  return { kind: 'dir', children: new Map(Object.entries(entries)), modified: day(age) };
}
function file(size: number, age = 1, text?: string): Node {
  return { kind: 'file', size, modified: day(age), text };
}

function locked(): Node {
  return { kind: 'dir', children: new Map([['Inbox.mbox', dir({}, 2)]]), modified: day(2), locked: true };
}

/** More than the API's 500-entry page, to show a cut-off listing. */
function exports(): Record<string, Node> {
  const out: Record<string, Node> = {};
  for (let n = 1; n <= 640; n += 1) out[`IMG_${String(4000 + n)}.heic`] = file(1_600_000 + ((n * 7919) % 2_400_000), 1 + (n % 30));
  return out;
}

function tree(): Node {
  return dir({
    Desktop: dir({ 'Screenshot 2026-10-18 at 9.41.12 AM.png': file(1_840_000, 2), 'Trip ideas.txt': file(412, 4, 'Lisbon in March?\nBook trains early.\n') }, 1),
    Documents: dir({
      'Budget 2027.numbers': file(286_000, 6),
      'Cover letter draft.pages': file(144_000, 12),
      Projects: dir({ 'portfolio-site': dir({ 'README.md': file(2_300, 8, '# Portfolio\n\nSynthetic demo project.\n'), 'package.json': file(860, 8, '{ "name": "portfolio-site" }\n') }, 8) }, 8),
      'notes.md': file(5_120, 0, '# Notes\n\n- Synthetic demo file.\n'),
    }, 0),
    Downloads: dir({
      'Xcode_17.xip': file(3_900_000_000, 3),
      'invoice-0142.pdf': file(96_000, 1),
      'setup-guide.pdf': file(2_400_000, 9),
      'Quarterly planning workshop notes and follow-up actions for the whole team (final, revised twice).pdf': file(1_220_000, 5),
      'design-assets.zip': file(48_500_000, 2),
    }, 1),
    Applications: dir({ 'Sketchpad.app': dir({ Contents: dir({}, 30) }, 30) }, 30),
    Library: dir({ Mail: locked(), Messages: locked(), Preferences: dir({ 'com.example.sketchpad.plist': file(1_400, 30) }, 30) }, 30),
    Movies: dir({}, 40),
    Music: dir({}, 40),
    Pictures: dir({ 'Photos Library.photoslibrary': dir({}, 2), Exports: dir(exports(), 1) }, 2),
    Public: dir({}, 60),
  }, 0);
}

export function createExtras(scenario: string): ExtrasState {
  const extras: ExtrasState = {
    prefs: {},
    defaults: { quota_gb: 500, max_disk_gb: 256, max_snapshots: 10, default_disk_gb: 128, reserve_gb: 40, snapshot_total_gb: 0 },
    overrides: {
      'u-sam': { quota_gb: 100, max_disk_gb: null, max_snapshots: 5 },
      'u-jo': { quota_gb: null, max_disk_gb: 128, max_snapshots: null },
    },
    files: {},
    clipboard: {},
    opened: [],
  };
  // alex's Macs use about 446 GB in the many-Macs scenarios.
  if (scenario === 'storage-full') extras.defaults = { ...extras.defaults, quota_gb: 400 };
  if (scenario === 'storage-unlimited') extras.defaults = { ...extras.defaults, quota_gb: 0, max_snapshots: 0 };
  return extras;
}

function hash(text: string): number {
  let h = 7;
  for (const ch of text) h = (h * 33 + ch.charCodeAt(0)) >>> 0;
  return h;
}

function macStorage(state: DemoState, instance: Instance): MacStorage {
  const snaps = state.snapshots[instance.id] ?? [];
  const local = instance.driver === 'qemu';
  const disk = local ? Math.round((18 + (hash(instance.id) % 40)) * GB + (hash(instance.name) % 900) * 1_000_000) : 0;
  const snapshots = local ? snaps.reduce((sum, snap) => sum + (snap.size_bytes ?? 0), 0) : 0;
  const memory = local && instance.state === 'hibernated' ? Math.round(instance.memory_mb * 1_048_576 * 0.62) : 0;
  return {
    instance_id: instance.id,
    name: instance.name,
    driver: instance.driver,
    state: instance.state,
    disk_gb: instance.disk_gb,
    used_bytes: disk + snapshots + memory,
    disk_bytes: disk,
    snapshots_bytes: snapshots,
    memory_bytes: memory,
    snapshot_count: local ? snaps.length : 0,
  };
}

function effective(extras: ExtrasState, userId: string): { limits: StorageLimits; sources: MyStorage['sources'] } {
  const o = extras.overrides[userId] ?? { quota_gb: null, max_disk_gb: null, max_snapshots: null };
  const pick = (key: keyof StorageLimits) => (o[key] === null ? extras.defaults[key] : o[key]);
  return {
    limits: { quota_gb: pick('quota_gb'), max_disk_gb: pick('max_disk_gb'), max_snapshots: pick('max_snapshots') },
    sources: {
      quota_gb: o.quota_gb === null ? 'default' : 'override',
      max_disk_gb: o.max_disk_gb === null ? 'default' : 'override',
      max_snapshots: o.max_snapshots === null ? 'default' : 'override',
    },
  };
}

function usage(state: DemoState, userId: string) {
  const macs = state.instances.filter((item) => state.owner[item.id] === userId && !state.hidden.has(item.id)).map((item) => macStorage(state, item));
  const used = macs.reduce((sum, mac) => sum + mac.used_bytes, 0);
  return { macs, used };
}

function myStorage(state: DemoState, userId: string): MyStorage {
  const { limits, sources } = effective(state.extras, userId);
  const { macs, used } = usage(state, userId);
  return { limits, sources, used_bytes: used, over_quota: limits.quota_gb > 0 && used >= limits.quota_gb * GB, macs, measured_at: new Date(Date.now() - 40_000).toISOString() };
}

function adminStorage(state: DemoState): AdminStorage {
  const people: PersonStorage[] = state.users.map((user) => {
    const { limits } = effective(state.extras, user.id);
    const { macs, used } = usage(state, user.id);
    return {
      user_id: user.id,
      username: user.username,
      display_name: user.display_name,
      role: user.role,
      used_bytes: used,
      mac_count: macs.length,
      overrides: state.extras.overrides[user.id] ?? { quota_gb: null, max_disk_gb: null, max_snapshots: null },
      limits,
      over_quota: limits.quota_gb > 0 && used >= limits.quota_gb * GB,
    };
  });
  const unlimited = people.some((person) => person.limits.quota_gb === 0);
  // Free space follows what the Macs use, so both storage pages agree.
  const total = 2_000_000_000_000;
  const images = 41_300_000_000;
  const system = 58_600_000_000;
  const macs = people.reduce((sum, person) => sum + person.used_bytes, 0);
  return {
    disk_total_bytes: total,
    disk_free_bytes: Math.max(0, total - images - system - macs),
    images_bytes: images,
    committed_gb: unlimited ? 0 : people.reduce((sum, person) => sum + person.limits.quota_gb, 0),
    defaults: state.extras.defaults,
    people,
    measured_at: new Date(Date.now() - 40_000).toISOString(),
  };
}

function resolve(path: string): string {
  const raw = path.trim() || '~';
  const full = raw === '~' ? HOME : raw.startsWith('~/') ? `${HOME}/${raw.slice(2)}` : raw;
  const parts: string[] = [];
  for (const part of full.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') parts.pop();
    else parts.push(part);
  }
  return `/${parts.join('/')}`;
}

function lookup(root: Node, path: string): Node | null {
  // Just enough of the disk above the home folder to walk up to it.
  if (path === '/Users') return { kind: 'dir', children: new Map([['dot', root], ['Shared', dir({}, 90)]]), modified: day(90) };
  if (path === '/') return dir({ Applications: dir({}, 20), Library: dir({}, 20), System: dir({}, 20), Users: dir({}, 90) }, 20);
  if (path !== HOME && !path.startsWith(`${HOME}/`)) return null;
  let node: Node = root;
  for (const part of path.slice(HOME.length).split('/').filter(Boolean)) {
    if (node.kind !== 'dir') return null;
    const next = node.children.get(part);
    if (!next) return null;
    node = next;
  }
  return node;
}

function item(path: string, node: Node): FileItem {
  return { name: path.split('/').pop() ?? path, path, kind: node.kind, size: node.kind === 'file' ? node.size : 0, modified: node.modified };
}

function freeName(folder: Node & { kind: 'dir' }, name: string): string {
  if (!folder.children.has(name)) return name;
  const dot = name.lastIndexOf('.');
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ''];
  for (let n = 2; ; n += 1) {
    const candidate = `${stem} ${n}${ext}`;
    if (!folder.children.has(candidate)) return candidate;
  }
}

const QUALITY = ['auto', 'sharp', 'balanced', 'fast'];

/** A real P-256 public key (the RFC 8292 example), so `pushManager.subscribe` accepts it. */
const DEMO_VAPID_KEY = 'BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs';

export function registerExtras(on: On, h: Helpers) {
  const running = (instance: Instance) => instance.state === 'ready' || instance.state === 'degraded';
  const needsRunning = (state: DemoState, id: string): { instance: Instance } | { error: Result } => {
    const instance = h.mine(state, id);
    if (!instance) return { error: h.fail(404, 'not_found', 'That Mac doesn’t exist, or it isn’t yours.') };
    if (!running(instance)) return { error: h.fail(503, 'unavailable', `Start ${instance.name} to use its files and clipboard.`) };
    return { instance };
  };
  const files = (state: DemoState, id: string) => (state.extras.files[id] ??= tree());

  // ---- Preferences ----
  on('GET', '/api/account/preferences', ({ state }) => h.json(state.extras.prefs[h.me(state)!.id] ?? DEFAULT_PREFERENCES));
  on('PUT', '/api/account/preferences', ({ state, body }) => {
    const id = h.me(state)!.id;
    const next: Preferences = { ...(state.extras.prefs[id] ?? DEFAULT_PREFERENCES) };
    const choose = <K extends keyof Preferences>(key: K, allowed: readonly string[]) => {
      if (!(key in body)) return null;
      if (!allowed.includes(String(body[key]))) return h.fail(400, 'invalid', 'That isn’t one of the choices.', {}, key);
      (next as Record<string, unknown>)[key] = body[key];
      return null;
    };
    const bad =
      choose('undo_seconds', ['0', '3', '5', '10']) ??
      choose('appearance', ['system', 'light', 'dark']) ??
      choose('start_page', ['home', 'last_mac']) ??
      choose('screen_mode', ['view', 'control']) ??
      choose('screen_quality', QUALITY) ??
      choose('screen_scale', ['fit', 'actual']);
    if (bad) return bad;
    for (const key of ['clipboard_sync', 'map_ctrl_to_cmd', 'approval_notifications', 'approval_sound'] as const) {
      if (key in body) next[key] = !!body[key];
    }
    state.extras.prefs[id] = next;
    return h.json(next);
  });

  // ---- Web Push ----
  // The demo can't push (no server key, no push service), but subscribing works so
  // the Notifications setting behaves. The key is a valid P-256 point, nothing more.
  on('GET', '/api/push/key', () => h.json({ public_key: DEMO_VAPID_KEY }));
  on('POST', '/api/push/subscriptions', ({ body }) => {
    const keys = body.keys as { p256dh?: unknown; auth?: unknown } | undefined;
    if (typeof body.endpoint !== 'string' || !body.endpoint.startsWith('https://')) return h.fail(400, 'invalid', 'This browser’s push service isn’t supported.', {}, 'endpoint');
    if (typeof keys?.p256dh !== 'string' || typeof keys.auth !== 'string') return h.fail(400, 'invalid', 'The browser’s push key isn’t valid.', {}, 'keys.p256dh');
    return h.json({});
  });
  on('DELETE', '/api/push/subscriptions', () => h.json({}));

  // ---- Storage ----
  on('GET', '/api/storage', ({ state }) => h.json(myStorage(state, h.me(state)!.id)));
  on('POST', '/api/instances/:id/storage/discard-memory', ({ state }, id) => {
    const instance = h.mine(state, id);
    if (!instance) return h.fail(404, 'not_found', 'That Mac doesn’t exist, or it isn’t yours.');
    if (instance.state !== 'hibernated') return h.fail(409, 'conflict', 'Only a hibernated Mac has saved memory to throw away.');
    h.update(state, instance, { state: 'stopped', state_detail: null });
    return h.json(instance);
  });
  on('GET', '/api/admin/storage', ({ state }) => h.admin(state) ?? h.json(adminStorage(state)));
  on('PUT', '/api/admin/storage/defaults', ({ state, body }) => {
    const gate = h.admin(state) ?? h.sensitive(state);
    if (gate) return gate;
    const next = { ...state.extras.defaults };
    const ranges: Record<keyof StorageDefaults, [number, number, string]> = {
      quota_gb: [0, 100_000, 'Choose 0 (no limit) to 100,000 GB.'],
      max_disk_gb: [64, 4096, 'Choose 64 GB to 4 TB.'],
      max_snapshots: [0, 100, 'Choose 0 (no limit) to 100 snapshots.'],
      default_disk_gb: [64, 4096, 'Choose 64 GB to 4 TB.'],
      reserve_gb: [5, 10_000, 'Keep at least 5 GB free.'],
    };
    for (const key of Object.keys(ranges) as (keyof StorageDefaults)[]) {
      if (!(key in body)) continue;
      const value = Number(body[key]);
      const [min, max, message] = ranges[key];
      if (!Number.isInteger(value) || value < min || value > max) return h.fail(400, 'invalid', message, {}, key);
      next[key] = value;
    }
    if ('snapshot_total_gb' in body) {
      const value = Number(body.snapshot_total_gb);
      if (!Number.isInteger(value) || value < 0 || value > 100_000) return h.fail(400, 'invalid', 'Choose 0 (no limit) to 100,000 GB.', {}, 'snapshot_total_gb');
      next.snapshot_total_gb = value;
    }
    if (next.default_disk_gb > next.max_disk_gb) return h.fail(400, 'invalid', 'The default disk can’t be larger than the largest disk allowed.', {}, 'default_disk_gb');
    state.extras.defaults = next;
    return h.json(adminStorage(state));
  });
  on('PUT', '/api/admin/users/:id/storage', ({ state, body }, userId) => {
    const gate = h.admin(state) ?? h.sensitive(state);
    if (gate) return gate;
    if (!state.users.some((user) => user.id === userId)) return h.fail(404, 'not_found', 'That person isn’t on this server.');
    const current = state.extras.overrides[userId] ?? { quota_gb: null, max_disk_gb: null, max_snapshots: null };
    const next = { ...current };
    for (const key of ['quota_gb', 'max_disk_gb', 'max_snapshots'] as const) {
      if (!(key in body)) continue;
      const raw = body[key];
      if (raw === null) {
        next[key] = null;
        continue;
      }
      const value = Number(raw);
      const min = key === 'max_disk_gb' ? 64 : 0;
      if (!Number.isInteger(value) || value < min || value > 100_000) return h.fail(400, 'invalid', key === 'max_disk_gb' ? 'Choose 64 GB or more.' : 'Choose 0 (no limit) or more.', {}, key);
      next[key] = value;
    }
    state.extras.overrides[userId] = next;
    return h.json(adminStorage(state));
  });

  // ---- Files ----
  on('GET', '/api/instances/:id/files', ({ state, query }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const path = resolve(query.get('path') ?? '~');
    const node = lookup(files(state, id), path);
    if (!node) return h.fail(404, 'not_found', `There’s no folder at ${path}.`);
    if (node.kind !== 'dir') return h.fail(400, 'invalid', `${path} is a file, not a folder.`, {}, 'path');
    if (node.locked && checked.instance.guest?.permissions.full_disk_access !== true) {
      return h.fail(403, 'forbidden', `macOS didn’t let Familiar open ${path}. It’s protected by privacy settings on the Mac.`);
    }
    const all = [...node.children.entries()]
      .map(([name, child]) => item(`${path === '/' ? '' : path}/${name}`, child))
      .sort((a, b) => (a.kind === 'dir' ? 0 : 1) - (b.kind === 'dir' ? 0 : 1) || a.name.localeCompare(b.name));
    const entries = all.slice(0, 500);
    const listing: FolderListing = { path, parent: path === HOME ? '/Users' : path === '/' ? null : path.slice(0, path.lastIndexOf('/')) || '/', entries, truncated: all.length > 500 };
    return h.json(listing);
  });
  on('GET', '/api/instances/:id/files/download', ({ state, query }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const path = resolve(query.get('path') ?? '');
    const node = lookup(files(state, id), path);
    if (!node || node.kind !== 'file') return h.fail(404, 'not_found', `There’s no file at ${path}.`);
    return { status: 200, blob: new Blob([node.text ?? `Synthetic demo file: ${path}\n`], { type: 'text/plain' }) };
  });
  on('POST', '/api/instances/:id/files/upload', ({ state, query, body }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const folderPath = resolve(query.get('path') ?? '~/Downloads');
    const folder = lookup(files(state, id), folderPath);
    if (!folder || folder.kind !== 'dir') return h.fail(404, 'not_found', `There’s no folder at ${folderPath}.`);
    const name = (query.get('name') ?? '').trim();
    if (!name || name.includes('/') || name === '.' || name === '..') return h.fail(400, 'invalid', 'That file name can’t be used on a Mac.', {}, 'name');
    const size = Number(body.__size ?? 0);
    if (size > 2 * GB) return h.fail(413, 'invalid', 'Files up to 2 GB can be uploaded here. For bigger files, use AirDrop or a shared folder on the Mac.');
    const final = freeName(folder, name);
    const node = file(size, 0);
    folder.children.set(final, node);
    return h.json(item(`${folderPath}/${final}`, node));
  });
  on('POST', '/api/instances/:id/files/folder', ({ state, body }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const path = resolve(String(body.path ?? ''));
    const parentPath = path.slice(0, path.lastIndexOf('/')) || '/';
    const parent = lookup(files(state, id), parentPath);
    const name = path.split('/').pop() ?? '';
    if (!parent || parent.kind !== 'dir') return h.fail(404, 'not_found', `There’s no folder at ${parentPath}.`);
    if (parent.children.has(name)) return h.fail(409, 'conflict', `“${name}” already exists here.`, {}, 'path');
    const node = dir({}, 0);
    parent.children.set(name, node);
    return h.json(item(path, node));
  });
  on('POST', '/api/instances/:id/files/trash', ({ state, body }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const path = resolve(String(body.path ?? ''));
    const parent = lookup(files(state, id), path.slice(0, path.lastIndexOf('/')) || '/');
    const name = path.split('/').pop() ?? '';
    if (path === HOME || !parent || parent.kind !== 'dir' || !parent.children.has(name)) return h.fail(404, 'not_found', `There’s nothing at ${path}.`);
    parent.children.delete(name);
    return h.json({});
  });

  // ---- Clipboard and links ----
  on('GET', '/api/instances/:id/clipboard', ({ state }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    return h.json({ text: state.extras.clipboard[id] ?? 'Synthetic text copied on the demo Mac' });
  });
  on('PUT', '/api/instances/:id/clipboard', ({ state, body }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    const text = String(body.text ?? '');
    if (text.length > 1_000_000) return h.fail(413, 'invalid', 'That’s too much text to paste at once (1 MB at most).', {}, 'text');
    state.extras.clipboard[id] = text;
    return h.json({});
  });
  on('POST', '/api/instances/:id/open', ({ state, body }, id) => {
    const checked = needsRunning(state, id);
    if ('error' in checked) return checked.error;
    let url: URL;
    try {
      url = new URL(String(body.url ?? '').trim());
    } catch {
      return h.fail(400, 'invalid', 'Enter a full web address, like https://example.com.', {}, 'url');
    }
    if (url.protocol !== 'https:' && url.protocol !== 'http:') return h.fail(400, 'invalid', 'Only web addresses (http or https) can be opened.', {}, 'url');
    state.extras.opened.push({ instance: id, url: url.href });
    return h.json({});
  });
}
