import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { agentChannelLabel, hourLabel, noteLines, offer, pollUntilBack, versionLabel, windowLabel, windowRange } from './updates';
import type { InstanceAgent, UpdatesStatus } from './types';

describe('versions and windows', () => {
  it('adds the short commit only for nightly builds that don’t already show it', () => {
    expect(versionLabel('3.1.0', 'e19a7c3f5d2b', 'stable')).toBe('3.1.0');
    expect(versionLabel('3.2.0-nightly.20261017', 'c41de09b7a3f', 'nightly')).toBe('3.2.0-nightly.20261017 (c41de09)');
    expect(versionLabel('3.2.0-nightly+c41de09', 'c41de09b7a3f', 'nightly')).toBe('3.2.0-nightly+c41de09');
    expect(versionLabel('3.1.0-dev', null, null)).toBe('3.1.0-dev');
  });

  it.each([
    [3, 'between 3:00 and 5:00 AM', '3:00 to 5:00 AM'],
    [11, 'between 11:00 AM and 1:00 PM', '11:00 AM to 1:00 PM'],
    [22, 'between 10:00 PM and 12:00 AM', '10:00 PM to 12:00 AM'],
    [23, 'between 11:00 PM and 1:00 AM', '11:00 PM to 1:00 AM'],
    [0, 'between 12:00 and 2:00 AM', '12:00 to 2:00 AM'],
  ])('a window starting at %i reads naturally', (hour, sentence, item) => {
    expect(windowLabel(hour)).toBe(sentence);
    expect(windowRange(hour)).toBe(item);
  });

  it('labels hours on a 12-hour clock', () => {
    expect([0, 3, 12, 15].map(hourLabel)).toEqual(['12:00 AM', '3:00 AM', '12:00 PM', '3:00 PM']);
  });
});

describe('release notes', () => {
  it('turns Markdown into plain lines without parsing anything', () => {
    const notes = '## What’s new\r\n- Faster approvals\n* <b>Bold</b> stays text   \n\n\n\n## Fixes\n+ One fix\n\n';
    expect(noteLines(notes)).toEqual([
      { text: 'What’s new', heading: true },
      { text: '• Faster approvals', heading: false },
      { text: '• <b>Bold</b> stays text', heading: false },
      { text: '', heading: false },
      { text: 'Fixes', heading: true },
      { text: '• One fix', heading: false },
    ]);
  });

  it('is empty for missing notes', () => {
    expect(noteLines(null)).toEqual([]);
    expect(noteLines('\n\n')).toEqual([]);
  });
});

const status = (patch: Partial<UpdatesStatus> = {}): UpdatesStatus => ({
  enabled: true,
  disabled_reason: null,
  current: { version: '3.0.0', commit: 'abc', channel: 'stable' },
  previous: null,
  settings: { channel: 'stable', auto_install: false, window_start_hour: 3, guest_default_channel: 'stable', guest_auto_install: true },
  available: null,
  last_checked_at: null,
  state: 'idle',
  detail: null,
  history: [],
  ...patch,
});
const release = { commit: 'def', published_at: '2026-10-17T00:00:00Z', notes: null, url: null };

describe('what the status card offers', () => {
  it('reads newer, the running build, and an older build after a channel switch', () => {
    expect(offer(status({ available: { ...release, version: '3.1.0', newer: true } }))).toEqual({ kind: 'newer', version: '3.1.0' });
    expect(offer(status({ available: { ...release, version: '3.0.0', newer: false } }))).toEqual({ kind: 'current' });
    expect(offer(status({ current: { version: '3.2.0-nightly.1', commit: 'x', channel: 'nightly' }, available: { ...release, version: '3.1.0', newer: false } }))).toEqual({ kind: 'switch', version: '3.1.0', channel: 'stable' });
  });

  it('offers nothing when updates are off or unavailable', () => {
    const available = { ...release, version: '3.1.0', newer: true };
    expect(offer(status({ available, enabled: false }))).toEqual({ kind: 'none' });
    expect(offer(status({ available, settings: { ...status().settings, channel: 'off' } }))).toEqual({ kind: 'none' });
    expect(offer(status())).toEqual({ kind: 'none' });
  });

  it('names the server setting a Mac resolves to only while it follows it', () => {
    const agent: InstanceAgent = { version: '3.0.0', channel: 'default', effective_channel: 'nightly', available: null, updating: false, can_update: true };
    expect(agentChannelLabel('default', agent)).toBe('Use Server Setting (Nightly)');
    expect(agentChannelLabel('default', { ...agent, channel: 'stable', effective_channel: 'stable' })).toBe('Use Server Setting');
    expect(agentChannelLabel('off', agent)).toBe('Off');
  });
});

describe('reconnecting after a restart', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('waits through the outage and resolves with the new version', async () => {
    const answers: (string | Error)[] = ['3.0.0', new Error('down'), new Error('down'), '3.1.0'];
    const probe = vi.fn(async () => {
      const next = answers.shift()!;
      if (next instanceof Error) throw next;
      return { version: next };
    });
    let back: { version: string } | null = null;
    void pollUntilBack({ probe, versionOf: (s) => s.version, from: '3.0.0', intervalMs: 1000 }).then((s) => (back = s));
    // The old process answering before it exits doesn't count.
    await vi.advanceTimersByTimeAsync(1000);
    expect(back).toBeNull();
    await vi.advanceTimersByTimeAsync(2000);
    expect(back).toBeNull();
    await vi.advanceTimersByTimeAsync(1000);
    expect(back).toEqual({ version: '3.1.0' });
    expect(probe).toHaveBeenCalledTimes(4);
  });

  it('accepts the same version once the server was down (it went back on its own)', async () => {
    const answers: (string | Error)[] = [new Error('down'), '3.0.0'];
    const probe = async () => {
      const next = answers.shift()!;
      if (next instanceof Error) throw next;
      return next;
    };
    const done = pollUntilBack({ probe, versionOf: (v) => v, from: '3.0.0', intervalMs: 500 });
    await vi.advanceTimersByTimeAsync(1000);
    await expect(done).resolves.toBe('3.0.0');
  });

  it('says once that it’s slow and keeps polling until stopped', async () => {
    const onSlow = vi.fn();
    const controller = new AbortController();
    const probe = vi.fn(async () => {
      throw new Error('down');
    });
    const done = pollUntilBack({ probe, versionOf: () => '', from: '3.0.0', intervalMs: 1000, slowAfterMs: 5000, onSlow, signal: controller.signal });
    const settled = done.catch((error: DOMException) => error.name);
    await vi.advanceTimersByTimeAsync(4000);
    expect(onSlow).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(4000);
    expect(onSlow).toHaveBeenCalledTimes(1);
    const calls = probe.mock.calls.length;
    controller.abort();
    await vi.advanceTimersByTimeAsync(5000);
    expect(probe.mock.calls.length).toBe(calls);
    await expect(settled).resolves.toBe('AbortError');
  });
});
