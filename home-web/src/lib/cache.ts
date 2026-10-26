import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { ApiError, abortRequests } from './api';

// A small shared cache: every screen reads server state through it, and live
// events write into it, so one update shows up everywhere at once.

export type Entry<T> = {
  data: T | undefined;
  error: ApiError | undefined;
  loading: boolean;
  updatedAt: number;
};

const EMPTY: Entry<never> = Object.freeze({ data: undefined, error: undefined, loading: false, updatedAt: 0 });

/** A fetcher may pass the signal on; the cache aborts it when the request is replaced or the cache cleared. */
export type Fetcher<T> = (signal: AbortSignal) => Promise<T>;

type Flight = { promise: Promise<unknown>; seq: number; startedAt: number; controller: AbortController };

/**
 * A request still running after this long no longer blocks new ones: the next
 * reader (a poll, a revisit) starts a fresh request. API calls time out sooner
 * than this; it guards fetchers that never settle.
 */
export const INFLIGHT_MAX_MS = 30_000;

const entries = new Map<string, Entry<unknown>>();
const inflight = new Map<string, Flight>();
const listeners = new Map<string, Set<() => void>>();
const fetchers = new Map<string, Fetcher<unknown>>();

// Every request start and every direct write takes the next tick of one clock.
// A response only lands when its request is the key's latest and nothing was
// written to the key after the request went out, so a slow read can't undo a
// newer change (a Pause toggle, a live event).
let clock = 0;
const lastWrite = new Map<string, number>();
const lastRequest = new Map<string, number>();
/** Bumped by clearCache so responses from before it are dropped. */
let generation = 0;

function notify(key: string) {
  listeners.get(key)?.forEach((listener) => listener());
}

function write<T>(key: string, patch: Partial<Entry<T>>) {
  const prev = (entries.get(key) as Entry<T> | undefined) ?? EMPTY;
  entries.set(key, { ...prev, ...patch });
  notify(key);
}

export function peek<T>(key: string): T | undefined {
  return entries.get(key)?.data as T | undefined;
}

export function setData<T>(key: string, data: T) {
  lastWrite.set(key, ++clock);
  write<T>(key, { data, error: undefined, updatedAt: Date.now() });
}

/** Update cached data in place. Does nothing when the key has no data yet. */
export function updateData<T>(key: string, fn: (data: T) => T) {
  const prev = entries.get(key)?.data as T | undefined;
  if (prev === undefined) return;
  lastWrite.set(key, ++clock);
  write<T>(key, { data: fn(prev), updatedAt: Date.now() });
}

export function fetchKey<T>(key: string, fetcher: Fetcher<T>): Promise<T> {
  fetchers.set(key, fetcher as Fetcher<unknown>);
  const running = inflight.get(key);
  if (running && Date.now() - running.startedAt < INFLIGHT_MAX_MS) return running.promise as Promise<T>;
  running?.controller.abort();

  const seq = ++clock;
  const gen = generation;
  const controller = new AbortController();
  lastRequest.set(key, seq);
  write<T>(key, { loading: true });
  const stale = () => gen !== generation || lastRequest.get(key) !== seq;
  /** A newer request for the key, if this one was replaced while running. */
  const newer = () => (gen === generation ? (inflight.get(key)?.promise as Promise<T> | undefined) : undefined);
  const overwritten = () => (lastWrite.get(key) ?? 0) > seq;

  const promise = new Promise<T>((resolve) => resolve(fetcher(controller.signal)))
    .then(
      (data): T | Promise<T> => {
        if (stale()) return newer() ?? data;
        if (overwritten()) {
          // Something newer landed while this was out; keep it.
          write<T>(key, { loading: false });
          return (peek<T>(key) ?? data) as T;
        }
        write<T>(key, { data, error: undefined, loading: false, updatedAt: Date.now() });
        return data;
      },
      (cause: unknown): Promise<T> => {
        if (stale()) return newer() ?? Promise.reject(cause);
        const error = cause instanceof ApiError ? cause : new ApiError('internal', 'Something went wrong. Try again.');
        write<T>(key, overwritten() ? { loading: false } : { error, loading: false });
        throw error;
      },
    )
    .finally(() => {
      if (inflight.get(key)?.seq === seq) inflight.delete(key);
    });
  inflight.set(key, { promise, seq, startedAt: Date.now(), controller });
  return promise;
}

/** Refetch every mounted key that starts with `prefix` (or all of them). */
export function invalidate(prefix = '') {
  for (const [key, fetcher] of fetchers) {
    if (key.startsWith(prefix) && listeners.get(key)?.size) void fetchKey(key, fetcher).catch(() => {});
  }
}

/** Forget everything (sign-out, a new sign-in) and stop the requests still out. */
export function clearCache() {
  generation += 1;
  for (const flight of inflight.values()) flight.controller.abort();
  abortRequests();
  entries.clear();
  inflight.clear();
  lastWrite.clear();
  lastRequest.clear();
  for (const key of listeners.keys()) notify(key);
}

function subscribe(key: string, listener: () => void) {
  let set = listeners.get(key);
  if (!set) listeners.set(key, (set = new Set()));
  set.add(listener);
  return () => {
    set.delete(listener);
  };
}

export function useEntry<T>(key: string | null): Entry<T> {
  const sub = useCallback((listener: () => void) => (key ? subscribe(key, listener) : () => {}), [key]);
  const snap = useCallback(() => ((key ? entries.get(key) : undefined) ?? EMPTY) as Entry<T>, [key]);
  return useSyncExternalStore(sub, snap, snap);
}

export type Query<T> = Entry<T> & { reload: () => Promise<T | undefined> };

/**
 * Read `key`, fetching it on mount when it is missing or older than `staleMs`.
 * Several components asking for one key share one request.
 */
export function useQuery<T>(key: string | null, fetcher: Fetcher<T>, options: { staleMs?: number; pollMs?: number } = {}): Query<T> {
  const entry = useEntry<T>(key);
  const { staleMs = 4000, pollMs } = options;

  useEffect(() => {
    if (!key) return;
    const existing = entries.get(key);
    if (!existing?.updatedAt || Date.now() - existing.updatedAt > staleMs) void fetchKey(key, fetcher).catch(() => {});
    else fetchers.set(key, fetcher as Fetcher<unknown>);
    if (!pollMs) return;
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') void fetchKey(key, fetcher).catch(() => {});
    }, pollMs);
    return () => clearInterval(timer);
    // The fetcher closes over the same ids the key encodes.
  }, [key, staleMs, pollMs]);

  const reload = useCallback(async () => {
    if (!key) return undefined;
    try {
      return await fetchKey(key, fetcher);
    } catch {
      return undefined;
    }
  }, [key]);

  return { ...entry, reload };
}

export const keys = {
  session: 'session',
  instances: 'instances',
  instance: (id: string) => `instance:${id}`,
  approvals: (id: string) => `approvals:${id}`,
  activity: (id: string, filter: 'all' | 'error') => `activity:${id}:${filter}`,
  fresh: (id: string) => `fresh:${id}`,
  live: (id: string) => `live:${id}`,
  apps: (id: string) => `apps:${id}`,
  permissions: (id: string) => `permissions:${id}`,
  approvalMode: (id: string) => `approval-mode:${id}`,
  review: (id: string, status: 'pending' | 'done') => `review:${id}:${status}`,
  snapshots: (id: string) => `snapshots:${id}`,
  connections: (id: string) => `connections:${id}`,
  account: 'account',
  security: 'security',
  overview: 'admin:overview',
  users: 'admin:users',
  adminInstances: 'admin:instances',
  invites: 'admin:invites',
  images: 'admin:images',
  updates: 'admin:updates',
  storage: 'storage',
  adminStorage: 'admin:storage',
  preferences: 'preferences',
  files: (id: string, path: string) => `files:${id}:${path}`,
};
