// Pure storage maths shared by the Storage page and Server > Storage, so both
// read the same numbers the same way (docs/home/API.md "Storage").
import { bytes, count } from './format';
import type { AdminStorage, LimitSource, MacStorage, MyStorage, StorageDefaults, StorageLimits } from './types';

export const GB = 1_000_000_000;

/** What a person's storage is made of. Order is the bar's order, largest kind first. */
export type Kind = 'disk' | 'snapshots' | 'memory' | 'other';

export const KIND_LABEL: Record<Kind, string> = {
  disk: 'macOS Disks',
  snapshots: 'Snapshots',
  memory: 'Saved Memory',
  other: 'Other',
};

export type Segment = { key: string; label: string; bytes: number; tone: Kind | 'images' | 'free' | 'system' };

/** Remote Macs keep their data on their own hardware; they never count. */
export const isLocal = (mac: MacStorage) => mac.driver !== 'remote';

/** One segment per kind, skipping empty ones. Anything the kinds don't explain shows as Other, never silently dropped. */
export function kindSegments(storage: Pick<MyStorage, 'macs' | 'used_bytes'>): Segment[] {
  const local = storage.macs.filter(isLocal);
  const sum = (pick: (mac: MacStorage) => number) => local.reduce((total, mac) => total + pick(mac), 0);
  const parts: Segment[] = [
    { key: 'disk', label: KIND_LABEL.disk, bytes: sum((mac) => mac.disk_bytes), tone: 'disk' },
    { key: 'snapshots', label: KIND_LABEL.snapshots, bytes: sum((mac) => mac.snapshots_bytes), tone: 'snapshots' },
    { key: 'memory', label: KIND_LABEL.memory, bytes: sum((mac) => mac.memory_bytes), tone: 'memory' },
  ];
  const explained = parts.reduce((total, part) => total + part.bytes, 0);
  const rest = storage.used_bytes - explained;
  // Measurements round; ignore anything under 0.1 GB.
  if (rest > GB / 10) parts.push({ key: 'other', label: KIND_LABEL.other, bytes: rest, tone: 'other' });
  return parts.filter((part) => part.bytes > 0);
}

/** A Mac's own breakdown, for its thin bar and detail. */
export function macSegments(mac: MacStorage): Segment[] {
  return kindSegments({ macs: [mac], used_bytes: mac.used_bytes });
}

export type Capacity = {
  used: number;
  /** null: no limit. */
  quota: number | null;
  /** What the bar's full width stands for: the quota, or the usage itself when it is larger or unlimited. */
  scale: number;
  /** Space left under the quota, never negative. */
  available: number | null;
  /** How far past the quota, 0 when under. */
  over: number;
};

export function capacity(used: number, quotaGb: number): Capacity {
  if (quotaGb <= 0) return { used, quota: null, scale: Math.max(used, 1), available: null, over: 0 };
  const quota = quotaGb * GB;
  return { used, quota, scale: Math.max(quota, used, 1), available: Math.max(0, quota - used), over: Math.max(0, used - quota) };
}

/** "400 GB", "1.5 TB", or "No limit" for 0. */
export function quotaLabel(gb: number): string {
  return gb <= 0 ? 'No limit' : bytes(gb * GB);
}
export function diskLabel(gb: number): string {
  return bytes(gb * GB);
}
export function snapshotsLabel(n: number): string {
  return n <= 0 ? 'No limit' : count(n, 'snapshot');
}

export function limitLabel(key: keyof StorageLimits | keyof StorageDefaults, value: number): string {
  if (key === 'quota_gb') return quotaLabel(value);
  if (key === 'max_snapshots') return snapshotsLabel(value);
  return diskLabel(value);
}

export function sourceLabel(source: LimitSource): string {
  return source === 'override' ? 'Set for you' : 'Server default';
}

/** A way to free space, with how much it frees. */
export type Win =
  | { kind: 'memory'; mac: MacStorage; bytes: number }
  | { kind: 'snapshots'; mac: MacStorage; bytes: number }
  | { kind: 'files'; mac: MacStorage; bytes: number };

/**
 * The biggest single actions that free space, largest first. Saved memory and
 * snapshots can be removed outright; a disk only shrinks by deleting files
 * inside the Mac, so it is offered only when nothing else would do.
 */
export function biggestWins(macs: MacStorage[], limit = 3): Win[] {
  const wins: Win[] = [];
  for (const mac of macs.filter(isLocal)) {
    if (mac.state === 'hibernated' && mac.memory_bytes > 0) wins.push({ kind: 'memory', mac, bytes: mac.memory_bytes });
    if (mac.snapshot_count > 0 && mac.snapshots_bytes > 0) wins.push({ kind: 'snapshots', mac, bytes: mac.snapshots_bytes });
  }
  wins.sort((a, b) => b.bytes - a.bytes);
  if (wins.length < limit) {
    const disks = macs.filter((mac) => isLocal(mac) && mac.disk_bytes > 0).sort((a, b) => b.disk_bytes - a.disk_bytes);
    for (const mac of disks.slice(0, limit - wins.length)) wins.push({ kind: 'files', mac, bytes: mac.disk_bytes });
  }
  return wins.slice(0, limit);
}

/** Local Macs largest first, then remote ones by name (they have nothing to measure). */
export function sortMacs(macs: MacStorage[]): MacStorage[] {
  return macs.toSorted((a, b) => {
    const local = Number(isLocal(b)) - Number(isLocal(a));
    if (local) return local;
    return b.used_bytes - a.used_bytes || a.name.localeCompare(b.name);
  });
}

/** The server's disk, split the way the admin needs to read it. */
export function serverSegments(admin: AdminStorage): Segment[] {
  const macs = admin.people.reduce((total, person) => total + person.used_bytes, 0);
  const used = Math.max(0, admin.disk_total_bytes - admin.disk_free_bytes);
  const other = Math.max(0, used - macs - admin.images_bytes);
  return [
    { key: 'macs', label: 'People’s Macs', bytes: macs, tone: 'disk' },
    { key: 'images', label: 'macOS Images', bytes: admin.images_bytes, tone: 'images' },
    { key: 'system', label: 'Other', bytes: other, tone: 'system' },
    { key: 'free', label: 'Available', bytes: admin.disk_free_bytes, tone: 'free' },
  ].filter((segment) => segment.bytes > 0) as Segment[];
}

export type Commitment =
  /** Someone has no quota, so promises can't be totalled. */
  | { kind: 'unbounded'; unlimited: number }
  | { kind: 'fits' | 'over'; committed: number; room: number };

/**
 * Quotas are promises, not reservations: Mac disks are thin, so the server only
 * spends what Macs actually write. `room` is what Macs could ever hold: what
 * they use now plus free space above the reserve.
 */
export function commitment(admin: AdminStorage): Commitment {
  const unlimited = admin.people.filter((person) => person.limits.quota_gb <= 0).length;
  if (unlimited > 0 || admin.committed_gb <= 0) return { kind: 'unbounded', unlimited };
  const macs = admin.people.reduce((total, person) => total + person.used_bytes, 0);
  const room = macs + Math.max(0, admin.disk_free_bytes - admin.defaults.reserve_gb * GB);
  const committed = admin.committed_gb * GB;
  return { kind: committed > room ? 'over' : 'fits', committed, room };
}

/** True when free space is under the reserve: new Macs and snapshots stop for everyone. */
export function underReserve(admin: AdminStorage): boolean {
  return admin.disk_free_bytes < admin.defaults.reserve_gb * GB;
}

// ---- Validation (contract ranges) ----

export const RANGES: Record<keyof StorageDefaults, { min: number; max: number; message: string }> = {
  quota_gb: { min: 0, max: 100_000, message: 'Use 1 to 100,000 GB, or turn on No Limit.' },
  max_disk_gb: { min: 64, max: 4096, message: 'Use 64 to 4,096 GB.' },
  default_disk_gb: { min: 64, max: 4096, message: 'Use 64 to 4,096 GB.' },
  max_snapshots: { min: 0, max: 100, message: 'Use 1 to 100, or turn on No Limit.' },
  reserve_gb: { min: 5, max: 10_000, message: 'Use 5 to 10,000 GB.' },
};

/** Parse a whole number typed by a person ("1,000" and " 200 " are fine). */
export function parseWhole(text: string): number | null {
  const clean = text.replace(/[,\s]/g, '');
  if (!/^\d+$/.test(clean)) return null;
  return Number(clean);
}

/** Error for one typed value, or null. `unlimited` fields reject 0 here: No Limit is its own switch. */
export function checkValue(key: keyof StorageDefaults, text: string): string | null {
  const value = parseWhole(text);
  const range = RANGES[key];
  if (value === null) return text.trim() ? 'Enter a whole number.' : 'Enter a number.';
  const min = key === 'quota_gb' || key === 'max_snapshots' ? 1 : range.min;
  if (value < min || value > range.max) return range.message;
  return null;
}

export type DefaultsDraft = {
  quota_unlimited: boolean;
  quota_gb: string;
  default_disk_gb: string;
  max_disk_gb: string;
  snapshots_unlimited: boolean;
  max_snapshots: string;
  reserve_gb: string;
};

export function draftFromDefaults(defaults: StorageDefaults): DefaultsDraft {
  return {
    quota_unlimited: defaults.quota_gb <= 0,
    quota_gb: defaults.quota_gb > 0 ? String(defaults.quota_gb) : '',
    default_disk_gb: String(defaults.default_disk_gb),
    max_disk_gb: String(defaults.max_disk_gb),
    snapshots_unlimited: defaults.max_snapshots <= 0,
    max_snapshots: defaults.max_snapshots > 0 ? String(defaults.max_snapshots) : '',
    reserve_gb: String(defaults.reserve_gb),
  };
}

export type DraftErrors = Partial<Record<keyof StorageDefaults, string>>;

/** Validate a defaults draft. Returns errors, or the body to send (only what changed). */
export function readDefaults(draft: DefaultsDraft, current: StorageDefaults): { errors: DraftErrors } | { body: Partial<StorageDefaults> } {
  const errors: DraftErrors = {};
  const next: StorageDefaults = { ...current };
  const take = (key: keyof StorageDefaults, text: string) => {
    const error = checkValue(key, text);
    if (error) errors[key] = error;
    else next[key] = parseWhole(text)!;
  };
  if (draft.quota_unlimited) next.quota_gb = 0;
  else take('quota_gb', draft.quota_gb);
  if (draft.snapshots_unlimited) next.max_snapshots = 0;
  else take('max_snapshots', draft.max_snapshots);
  take('default_disk_gb', draft.default_disk_gb);
  take('max_disk_gb', draft.max_disk_gb);
  take('reserve_gb', draft.reserve_gb);
  if (!errors.default_disk_gb && !errors.max_disk_gb && next.default_disk_gb > next.max_disk_gb) {
    errors.default_disk_gb = `Can’t be larger than the largest disk allowed (${diskLabel(next.max_disk_gb)}).`;
  }
  if (Object.keys(errors).length) return { errors };
  const body: Partial<StorageDefaults> = {};
  for (const key of Object.keys(next) as (keyof StorageDefaults)[]) if (next[key] !== current[key]) body[key] = next[key];
  return { body };
}

export const LIMIT_KEYS = ['quota_gb', 'max_disk_gb', 'max_snapshots'] as const satisfies readonly (keyof StorageLimits)[];
export type LimitKey = (typeof LIMIT_KEYS)[number];

/** One person's limit while it is being edited: the server default, or their own value. */
export type OverrideDraft = { custom: boolean; unlimited: boolean; text: string };

export function draftFromOverride(key: LimitKey, value: number | null, fallback: number): OverrideDraft {
  const shown = value ?? fallback;
  const unlimitable = key !== 'max_disk_gb';
  return { custom: value !== null, unlimited: unlimitable && shown <= 0, text: shown > 0 ? String(shown) : '' };
}

/** Validate one person's overrides. Returns errors, or the body to send (only what changed; null clears). */
export function readOverrides(
  drafts: Record<LimitKey, OverrideDraft>,
  current: Record<LimitKey, number | null>,
): { errors: Partial<Record<LimitKey, string>> } | { body: Partial<Record<LimitKey, number | null>> } {
  const errors: Partial<Record<LimitKey, string>> = {};
  const body: Partial<Record<LimitKey, number | null>> = {};
  for (const key of LIMIT_KEYS) {
    const draft = drafts[key];
    let value: number | null = null;
    if (draft.custom) {
      if (key !== 'max_disk_gb' && draft.unlimited) value = 0;
      else {
        const error = checkValue(key, draft.text);
        if (error) {
          errors[key] = error;
          continue;
        }
        value = parseWhole(draft.text);
      }
    }
    if (value !== current[key]) body[key] = value;
  }
  return Object.keys(errors).length ? { errors } : { body };
}
