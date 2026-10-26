import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, READ_TIMEOUT_MS, api } from './api';
import { INFLIGHT_MAX_MS, clearCache, fetchKey, keys, peek, setData, updateData } from './cache';
import { transport } from './transport';
import type { Instance } from './types';

const mac = (patch: Partial<Instance> = {}) => ({ id: 'm1', name: 'Mac', paused: false, ...patch }) as Instance;
const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });

/** A request the test answers by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const fetchOriginal = transport.fetch;
beforeEach(() => {
  vi.useFakeTimers();
  clearCache();
});
afterEach(() => {
  transport.fetch = fetchOriginal;
  vi.useRealTimers();
});

describe('a hung request', () => {
  it('times out instead of waiting forever, even when the transport ignores the signal', async () => {
    transport.fetch = () => new Promise(() => {});
    const result = api.instance('m1').catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(READ_TIMEOUT_MS - 1);
    expect(await Promise.race([result, 'pending'])).toBe('pending');
    await vi.advanceTimersByTimeAsync(1);
    const error = await result;
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe('timeout');
  });

  it('lands in the cache as an error, so the next read starts a new request', async () => {
    let calls = 0;
    transport.fetch = () => {
      calls += 1;
      return calls === 1 ? new Promise(() => {}) : Promise.resolve(json(mac({ name: 'Back' })));
    };
    const first = fetchKey(keys.instance('m1'), () => api.instance('m1')).catch(() => {});
    await vi.advanceTimersByTimeAsync(READ_TIMEOUT_MS);
    await first;
    await fetchKey(keys.instance('m1'), () => api.instance('m1'));
    expect(calls).toBe(2);
    expect(peek<Instance>(keys.instance('m1'))?.name).toBe('Back');
  });

  it('stops blocking new requests once it has run too long, and its late answer is dropped', async () => {
    const stuck = deferred<Instance>();
    const fetcher = vi.fn().mockReturnValueOnce(stuck.promise).mockResolvedValueOnce(mac({ name: 'Fresh' }));
    void fetchKey('instance:m1', fetcher);
    // Inside the window, readers share the running request.
    void fetchKey('instance:m1', fetcher);
    expect(fetcher).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(INFLIGHT_MAX_MS);
    await fetchKey('instance:m1', fetcher);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(fetcher.mock.calls[0]![0].aborted).toBe(true);
    stuck.resolve(mac({ name: 'Stale' }));
    await vi.advanceTimersByTimeAsync(0);
    expect(peek<Instance>('instance:m1')?.name).toBe('Fresh');
  });
});

describe('a slow read', () => {
  it('doesn’t undo a change written while it was out', async () => {
    const slow = deferred<Instance>();
    setData(keys.instance('m1'), mac());
    const read = fetchKey(keys.instance('m1'), () => slow.promise);
    // The Pause toggle answers first.
    setData(keys.instance('m1'), mac({ paused: true }));
    slow.resolve(mac({ paused: false }));
    expect((await read).paused).toBe(true);
    expect(peek<Instance>(keys.instance('m1'))?.paused).toBe(true);
  });

  it('doesn’t undo an in-place update either (a live event patching the list)', async () => {
    const slow = deferred<Instance[]>();
    setData(keys.instances, [mac()]);
    const read = fetchKey(keys.instances, () => slow.promise);
    updateData<Instance[]>(keys.instances, (list) => list.map((item) => ({ ...item, paused: true })));
    slow.resolve([mac({ paused: false })]);
    await read;
    expect(peek<Instance[]>(keys.instances)?.[0]?.paused).toBe(true);
  });

  it('still lands when nothing newer was written', async () => {
    setData(keys.instance('m1'), mac());
    await fetchKey(keys.instance('m1'), async () => mac({ name: 'Renamed' }));
    expect(peek<Instance>(keys.instance('m1'))?.name).toBe('Renamed');
  });

  it('drops a failure that arrives after a newer change', async () => {
    const slow = deferred<Instance>();
    const read = fetchKey(keys.instance('m1'), () => slow.promise).catch(() => {});
    setData(keys.instance('m1'), mac({ paused: true }));
    slow.reject(new ApiError('timeout', 'slow'));
    await read;
    expect(peek<Instance>(keys.instance('m1'))?.paused).toBe(true);
  });
});

describe('clearCache', () => {
  it('aborts requests in flight and ignores what they bring back', async () => {
    const slow = deferred<Instance>();
    let signal: AbortSignal | undefined;
    const read = fetchKey(keys.instance('m1'), (s) => {
      signal = s;
      return slow.promise;
    }).catch(() => {});
    clearCache();
    expect(signal?.aborted).toBe(true);
    slow.resolve(mac({ name: 'Previous account' }));
    await read;
    expect(peek(keys.instance('m1'))).toBeUndefined();
  });

  it('aborts API requests too', async () => {
    let signal: AbortSignal | undefined;
    transport.fetch = (_url, init) => {
      signal = init.signal ?? undefined;
      return new Promise(() => {});
    };
    const read = api.account().catch((error: unknown) => error);
    clearCache();
    expect(signal?.aborted).toBe(true);
    expect(await read).toBeInstanceOf(DOMException);
  });
});
