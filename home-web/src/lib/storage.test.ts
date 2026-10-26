import { describe, expect, it } from 'vitest';
import {
  GB, biggestWins, capacity, checkValue, commitment, draftFromDefaults, draftFromOverride, kindSegments, readDefaults, readOverrides, serverSegments, sortMacs,
} from './storage';
import type { AdminStorage, MacStorage, PersonStorage, StorageDefaults } from './types';

const mac = (over: Partial<MacStorage> = {}): MacStorage => {
  const base: MacStorage = {
    instance_id: 'm', name: 'Mac', driver: 'qemu', state: 'ready', disk_gb: 128,
    used_bytes: 0, disk_bytes: 20 * GB, snapshots_bytes: 0, memory_bytes: 0, snapshot_count: 0, ...over,
  };
  return { ...base, used_bytes: over.used_bytes ?? base.disk_bytes + base.snapshots_bytes + base.memory_bytes };
};
const defaults: StorageDefaults = { quota_gb: 200, max_disk_gb: 256, max_snapshots: 10, default_disk_gb: 128, reserve_gb: 40 };
const person = (used: number, quota: number): PersonStorage => ({
  user_id: `u${used}${quota}`, username: 'p', display_name: 'P', role: 'member', used_bytes: used * GB, mac_count: 1,
  overrides: { quota_gb: null, max_disk_gb: null, max_snapshots: null }, limits: { quota_gb: quota, max_disk_gb: 256, max_snapshots: 10 }, over_quota: false,
});
const admin = (people: PersonStorage[], over: Partial<AdminStorage> = {}): AdminStorage => ({
  disk_total_bytes: 2000 * GB, disk_free_bytes: 1000 * GB, images_bytes: 40 * GB,
  committed_gb: people.some((p) => p.limits.quota_gb === 0) ? 0 : people.reduce((sum, p) => sum + p.limits.quota_gb, 0),
  defaults, people, measured_at: '2026-10-20T00:00:00Z', ...over,
});

describe('typed limits', () => {
  it('rejects values outside the contract ranges, with the range in the message', () => {
    expect(checkValue('max_disk_gb', '63')).toMatch(/64 to 4,096/);
    expect(checkValue('max_disk_gb', '4097')).toMatch(/64 to 4,096/);
    expect(checkValue('reserve_gb', '4')).toMatch(/5 to 10,000/);
    expect(checkValue('max_snapshots', '101')).toMatch(/1 to 100/);
    expect(checkValue('quota_gb', '100001')).toMatch(/100,000/);
  });
  it('rejects 0 where No Limit is a separate switch, and anything that isn’t a whole number', () => {
    expect(checkValue('quota_gb', '0')).toMatch(/No Limit/);
    expect(checkValue('max_snapshots', '0')).toMatch(/No Limit/);
    expect(checkValue('quota_gb', '12.5')).toBe('Enter a whole number.');
    expect(checkValue('quota_gb', '-3')).toBe('Enter a whole number.');
    expect(checkValue('quota_gb', '  ')).toBe('Enter a number.');
  });
  it('accepts edges and thousands separators', () => {
    expect(checkValue('max_disk_gb', '64')).toBeNull();
    expect(checkValue('max_disk_gb', '4,096')).toBeNull();
    expect(checkValue('quota_gb', ' 1 000 ')).toBeNull();
  });
});

describe('readDefaults', () => {
  it('refuses a default disk larger than the largest disk allowed', () => {
    const draft = { ...draftFromDefaults(defaults), default_disk_gb: '300' };
    expect(readDefaults(draft, defaults)).toEqual({ errors: { default_disk_gb: expect.stringMatching(/256 GB/) } });
  });
  it('reports every bad field at once', () => {
    const draft = { ...draftFromDefaults(defaults), quota_gb: 'lots', reserve_gb: '1' };
    const result = readDefaults(draft, defaults);
    expect('errors' in result && Object.keys(result.errors).sort()).toEqual(['quota_gb', 'reserve_gb']);
  });
  it('sends only what changed, with No Limit as 0', () => {
    const draft = { ...draftFromDefaults(defaults), quota_unlimited: true, reserve_gb: '60' };
    expect(readDefaults(draft, defaults)).toEqual({ body: { quota_gb: 0, reserve_gb: 60 } });
    expect(readDefaults(draftFromDefaults(defaults), defaults)).toEqual({ body: {} });
  });
  it('round-trips an unlimited default without inventing a number', () => {
    const open = { ...defaults, quota_gb: 0, max_snapshots: 0 };
    expect(readDefaults(draftFromDefaults(open), open)).toEqual({ body: {} });
  });
});

describe('readOverrides', () => {
  const current = { quota_gb: 100, max_disk_gb: null, max_snapshots: null };
  const drafts = () => ({
    quota_gb: draftFromOverride('quota_gb', 100, 200),
    max_disk_gb: draftFromOverride('max_disk_gb', null, 256),
    max_snapshots: draftFromOverride('max_snapshots', null, 10),
  });
  it('blocks an out-of-range custom value', () => {
    const next = { ...drafts(), max_disk_gb: { custom: true, unlimited: false, text: '5000' } };
    expect(readOverrides(next, current)).toEqual({ errors: { max_disk_gb: expect.stringMatching(/64 to 4,096/) } });
  });
  it('sends null to go back to the server default', () => {
    const next = { ...drafts(), quota_gb: { ...drafts().quota_gb, custom: false } };
    expect(readOverrides(next, current)).toEqual({ body: { quota_gb: null } });
  });
  it('sends a custom No Limit as 0 and leaves untouched fields out', () => {
    const next = { ...drafts(), max_snapshots: { custom: true, unlimited: true, text: '' } };
    expect(readOverrides(next, current)).toEqual({ body: { max_snapshots: 0 } });
    expect(readOverrides(drafts(), current)).toEqual({ body: {} });
  });
});

describe('capacity', () => {
  it('has no ceiling when there is no limit', () => {
    expect(capacity(300 * GB, 0)).toEqual({ used: 300 * GB, quota: null, scale: 300 * GB, available: null, over: 0 });
  });
  it('stretches the scale past the quota when over, and never reports negative space', () => {
    const over = capacity(446 * GB, 400);
    expect(over.scale).toBe(446 * GB);
    expect(over.over).toBe(46 * GB);
    expect(over.available).toBe(0);
  });
});

describe('kindSegments', () => {
  it('ignores remote Macs and shows unexplained usage as Other instead of dropping it', () => {
    const macs = [mac({ disk_bytes: 30 * GB, snapshots_bytes: 5 * GB }), mac({ driver: 'remote', disk_bytes: 99 * GB })];
    const segments = kindSegments({ macs, used_bytes: 40 * GB });
    expect(segments.map((s) => [s.key, s.bytes / GB])).toEqual([['disk', 30], ['snapshots', 5], ['other', 5]]);
  });
  it('sums to what the API says was used', () => {
    const macs = [mac({ disk_bytes: 30 * GB, memory_bytes: 3 * GB }), mac({ disk_bytes: 10 * GB, snapshots_bytes: 2 * GB })];
    const used = macs.reduce((sum, m) => sum + m.used_bytes, 0);
    expect(kindSegments({ macs, used_bytes: used }).reduce((sum, s) => sum + s.bytes, 0)).toBe(used);
  });
});

describe('biggestWins', () => {
  it('offers saved memory only for a hibernated Mac, and ranks by space freed', () => {
    const wins = biggestWins([
      mac({ instance_id: 'awake', memory_bytes: 50 * GB }),
      mac({ instance_id: 'asleep', state: 'hibernated', memory_bytes: 5 * GB }),
      mac({ instance_id: 'snaps', snapshot_count: 3, snapshots_bytes: 12 * GB }),
    ]);
    expect(wins.map((w) => [w.kind, w.mac.instance_id])).toEqual([['snapshots', 'snaps'], ['memory', 'asleep'], ['files', 'awake']]);
  });
  it('falls back to the largest disks when nothing can be removed outright', () => {
    const wins = biggestWins([mac({ instance_id: 'a', disk_bytes: 10 * GB }), mac({ instance_id: 'b', disk_bytes: 40 * GB }), mac({ driver: 'remote', instance_id: 'r', disk_bytes: 90 * GB })], 2);
    expect(wins.map((w) => w.mac.instance_id)).toEqual(['b', 'a']);
  });
});

it('sortMacs puts the biggest local Mac first and remote Macs last', () => {
  const sorted = sortMacs([mac({ instance_id: 'r', driver: 'remote' }), mac({ instance_id: 's', disk_bytes: 5 * GB }), mac({ instance_id: 'l', disk_bytes: 50 * GB })]);
  expect(sorted.map((m) => m.instance_id)).toEqual(['l', 's', 'r']);
});

describe('server disk', () => {
  it('splits the disk into parts that add up to its size', () => {
    const data = admin([person(300, 500), person(100, 500)]);
    const segments = serverSegments(data);
    expect(segments.reduce((sum, s) => sum + s.bytes, 0)).toBe(data.disk_total_bytes);
    expect(segments.find((s) => s.key === 'system')?.bytes).toBe(2000 * GB - 1000 * GB - 400 * GB - 40 * GB);
  });
  it('can’t total promises when anyone has no quota', () => {
    expect(commitment(admin([person(10, 0), person(10, 500)]))).toEqual({ kind: 'unbounded', unlimited: 1 });
  });
  it('flags quotas that promise more than Macs could ever hold above the reserve', () => {
    // Room: 400 used + (1000 free - 40 reserve) = 1360 GB.
    expect(commitment(admin([person(300, 700), person(100, 660)]))).toMatchObject({ kind: 'fits', room: 1360 * GB });
    expect(commitment(admin([person(300, 700), person(100, 661)]))).toMatchObject({ kind: 'over' });
  });
});
