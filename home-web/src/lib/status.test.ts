import { describe, expect, it } from 'vitest';
import { instanceInfo, isLockedOff, powerMenu, progressOf, stateDetail } from './status';
import type { Instance } from './types';

type Mac = Pick<Instance, 'state' | 'state_detail' | 'locked' | 'encrypt_progress' | 'driver'>;
const mac = (patch: Partial<Mac>): Mac => ({ state: 'stopped', state_detail: null, locked: false, encrypt_progress: null, driver: 'qemu', ...patch });

describe('locked Macs', () => {
  it('read as Locked only while off, with no power action but shutting down a hibernated one', () => {
    expect(isLockedOff(mac({ locked: true }))).toBe(true);
    expect(instanceInfo(mac({ locked: true })).label).toBe('Locked');
    expect(stateDetail(mac({ locked: true, state_detail: 'Locked. Sign in to unlock.' }))).toBe('Sign in to unlock.');
    expect(powerMenu(mac({ locked: true }))).toEqual([]);
    expect(powerMenu(mac({ locked: true, state: 'hibernated' }))).toEqual(['stop']);
    // A running Mac keeps running after a restart; it reads as usual.
    expect(isLockedOff(mac({ locked: true, state: 'ready' }))).toBe(false);
    expect(instanceInfo(mac({ locked: true, state: 'ready' })).label).toBe('Ready');
    expect(instanceInfo(mac({ locked: false })).label).toBe('Off');
  });
});

describe('encrypting', () => {
  it('shows its progress and offers no power actions', () => {
    const m = mac({ state: 'encrypting', encrypt_progress: 0.42 });
    expect(stateDetail(m)).toBe('Encrypting… 42%');
    expect(progressOf(m)).toBe(0.42);
    expect(progressOf(mac({ state: 'encrypting' }))).toBeNull();
    expect(powerMenu(m)).toEqual([]);
    expect(instanceInfo(m).transitional).toBe(true);
  });
});
