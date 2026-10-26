import { describe, expect, it } from 'vitest';
import { bytes, countdown, duration, expiresIn, groupSecret, initials, memory, minutes, uptime, when, whenInline } from './format';

const now = new Date('2026-10-19T15:00:00').getTime();
const at = (ms: number) => new Date(now - ms).toISOString();

describe('when', () => {
  it('reads like the clock in Messages', () => {
    expect(when(at(10_000), now)).toBe('Just now');
    expect(when(at(4 * 60_000), now)).toBe('4 minutes ago');
    expect(when(at(3 * 3_600_000), now)).toBe('12:00 PM');
    expect(when(at(24 * 3_600_000), now)).toBe('Yesterday');
    expect(when(at(30 * 24 * 3_600_000), now)).toBe('Sep 19, 2026');
    expect(when(null, now)).toBe('');
  });

  it('fits mid-sentence without shouting or lowercasing dates', () => {
    expect(whenInline(at(10_000), now)).toBe('just now');
    expect(whenInline(at(3 * 3_600_000), now)).toBe('at 12:00 PM');
    expect(whenInline(at(30 * 24 * 3_600_000), now)).toBe('on Sep 19, 2026');
  });
});

describe('formatters', () => {
  it('formats sizes in decimal units like macOS', () => {
    expect(bytes(61_400_000_000)).toBe('61.4 GB');
    expect(bytes(2_000_000_000_000)).toBe('2 TB');
    expect(bytes(512)).toBe('512 bytes');
    expect(bytes(null)).toBe('');
  });

  it('formats memory, timers and durations', () => {
    expect(memory(8192)).toBe('8 GB');
    expect(memory(12_800)).toBe('12.5 GB');
    expect(minutes(0)).toBe('Never');
    expect(minutes(30)).toBe('30 minutes');
    expect(minutes(120)).toBe('2 hours');
    expect(duration(845)).toBe('845 ms');
    expect(duration(1234)).toBe('1.2 s');
    expect(uptime(5 * 3600 + 720)).toBe('5 hours, 12 minutes');
    expect(uptime(86_400)).toBe('1 day');
  });

  it('counts down approvals and lockouts', () => {
    expect(expiresIn(new Date(now + 42_000).toISOString(), now)).toBe('42 s left');
    expect(expiresIn(new Date(now - 1).toISOString(), now)).toBe('Expired');
    expect(countdown(42.2)).toBe('0:43');
    expect(countdown(725)).toBe('12:05');
  });

  it('makes initials and readable setup keys', () => {
    expect(initials('Alex Rivera')).toBe('AR');
    expect(initials('  cher ')).toBe('C');
    expect(groupSecret('JBSWY3DPEHPK3PXP')).toBe('JBSW Y3DP EHPK 3PXP');
  });
});
