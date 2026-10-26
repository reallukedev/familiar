import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { clearCache, keys, peek, setData } from './cache';
import { ACTIVITY_LIMIT, liveHooks, putActivity, putApproval, sortApprovals, subscribeLive } from './live';
import { transport, type EventSourceLike } from './transport';
import type { Activity, ActivityPage, Approval, Instance } from './types';

class FakeSource implements EventSourceLike {
  static all: FakeSource[] = [];
  readyState = 0;
  onerror: ((event: Event) => void) | null = null;
  onopen: ((event: Event) => void) | null = null;
  listeners = new Map<string, (event: MessageEvent) => void>();
  closed = false;
  constructor(readonly url: string) {
    FakeSource.all.push(this);
  }
  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    this.listeners.set(type, listener);
  }
  close() {
    this.closed = true;
    this.readyState = 2;
  }
  open() {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }
  send(type: string, data: unknown) {
    this.listeners.get(type)?.(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
  die() {
    this.readyState = 2;
    this.onerror?.(new Event('error'));
  }
}

const mac = (patch: Partial<Instance> = {}) => ({ id: 'm1', name: 'Mac', state: 'stopped', counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 }, ...patch }) as Instance;

beforeEach(() => {
  vi.useFakeTimers();
  FakeSource.all = [];
  transport.eventSource = (url) => new FakeSource(url);
  transport.fetch = async () => new Response(JSON.stringify([]), { status: 200 });
  clearCache();
});
afterEach(() => vi.useRealTimers());

describe('live events', () => {
  it('writes instance events into the Mac and the Mac list', () => {
    setData(keys.instances, [mac()]);
    const stop = subscribeLive('m1');
    FakeSource.all[0]!.open();
    FakeSource.all[0]!.send('instance', mac({ state: 'ready' }));
    expect(peek<Instance>(keys.instance('m1'))?.state).toBe('ready');
    expect(peek<Instance[]>(keys.instances)?.[0]?.state).toBe('ready');
    stop();
  });

  it('shares one stream between subscribers and closes it with the last one', () => {
    const a = subscribeLive('m1');
    const b = subscribeLive('m1');
    expect(FakeSource.all).toHaveLength(1);
    a();
    expect(FakeSource.all[0]!.closed).toBe(false);
    b();
    expect(FakeSource.all[0]!.closed).toBe(true);
  });

  it('reconnects with growing delays after the server closes the stream', () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stop = subscribeLive('m1');
    FakeSource.all[0]!.die();
    expect(peek(keys.live('m1'))).toBe('reconnecting');
    vi.advanceTimersByTime(999);
    expect(FakeSource.all).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSource.all).toHaveLength(2);
    FakeSource.all[1]!.die();
    vi.advanceTimersByTime(1999);
    expect(FakeSource.all).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeSource.all).toHaveLength(3);
    FakeSource.all[2]!.open();
    expect(peek(keys.live('m1'))).toBe('open');
    stop();
    vi.restoreAllMocks();
  });

  it('asks the connection layer to check the server once every stream drops', () => {
    const allDown = vi.fn();
    liveHooks.allDown = allDown;
    const a = subscribeLive('m1');
    const b = subscribeLive('m2');
    FakeSource.all[0]!.open();
    FakeSource.all[1]!.open();
    FakeSource.all[0]!.die();
    expect(allDown).not.toHaveBeenCalled();
    FakeSource.all[1]!.die();
    expect(allDown).toHaveBeenCalledTimes(1);
    a();
    b();
    liveHooks.allDown = () => {};
  });

  it('stops retrying once unsubscribed', () => {
    const stop = subscribeLive('m1');
    FakeSource.all[0]!.die();
    stop();
    vi.advanceTimersByTime(60_000);
    expect(FakeSource.all).toHaveLength(1);
  });
});

describe('approvals', () => {
  const approval = (id: string, status: Approval['status'], created: string): Approval => ({ id, instance_id: 'm1', tool: 't', title: 'T', summary: 's', client: null, created_at: created, expires_at: '2999-01-01T00:00:00Z', status });

  it('puts pending requests first, newest first', () => {
    const sorted = sortApprovals([approval('a', 'approved', '2026-01-03'), approval('b', 'pending', '2026-01-01'), approval('c', 'pending', '2026-01-02')]);
    expect(sorted.map((item) => item.id)).toEqual(['c', 'b', 'a']);
  });

  it('leaves expired requests out of the waiting count', () => {
    vi.setSystemTime(new Date('2026-01-01T12:00:00Z'));
    setData(keys.instance('m1'), mac());
    setData(keys.approvals('m1'), [{ ...approval('old', 'pending', '2026-01-01T10:00:00Z'), expires_at: '2026-01-01T11:00:00Z' }]);
    putApproval({ ...approval('new', 'pending', '2026-01-01T11:59:00Z'), expires_at: '2026-01-01T12:05:00Z' });
    expect(peek<Instance>(keys.instance('m1'))?.counts.pending_approvals).toBe(1);
  });

  it('keeps the Mac’s waiting count in step as requests arrive and resolve', () => {
    setData(keys.instance('m1'), mac());
    setData(keys.approvals('m1'), []);
    putApproval(approval('x', 'pending', '2026-01-01'));
    expect(peek<Instance>(keys.instance('m1'))?.counts.pending_approvals).toBe(1);
    putApproval(approval('x', 'approved', '2026-01-01'));
    expect(peek<Instance>(keys.instance('m1'))?.counts.pending_approvals).toBe(0);
  });
});

describe('activity', () => {
  const item = (n: number): Activity => ({ id: `a${n}`, at: '2026-01-01', tool: 't', title: 'T', outcome: 'ok', duration_ms: 1, detail: null, error_code: null, client: null });

  it('keeps at most the newest rows in memory and pages from where it cut', () => {
    setData<ActivityPage>(keys.activity('m1', 'all'), { items: Array.from({ length: ACTIVITY_LIMIT }, (_, i) => item(i)), next: null });
    putActivity('m1', item(-1));
    const page = peek<ActivityPage>(keys.activity('m1', 'all'))!;
    expect(page.items).toHaveLength(ACTIVITY_LIMIT);
    expect(page.items[0]!.id).toBe('a-1');
    expect(page.next).toBe(page.items.at(-1)!.id);
  });
});
