import { useEffect } from 'react';
import { api } from './api';
import { backoff } from './backoff';
import { fetchKey, invalidate, keys, peek, setData, updateData } from './cache';
import { transport, type EventSourceLike } from './transport';
import type { Activity, ActivityPage, Approval, ImageStatus, Instance, UpdatesStatus } from './types';

export type LiveState = 'connecting' | 'open' | 'reconnecting';

// ---- Writers shared by events and mutations ----------------------------------

export function putInstance(instance: Instance) {
  setData(keys.instance(instance.id), instance);
  updateData<Instance[]>(keys.instances, (list) => {
    const index = list.findIndex((item) => item.id === instance.id);
    if (index === -1) return [...list, instance];
    const next = list.slice();
    next[index] = instance;
    return next;
  });
}

export function removeInstance(id: string) {
  updateData<Instance[]>(keys.instances, (list) => list.filter((item) => item.id !== id));
}

function upsertById<T extends { id: string }>(list: T[], item: T, prepend: boolean): T[] {
  const index = list.findIndex((existing) => existing.id === item.id);
  if (index === -1) return prepend ? [item, ...list] : [...list, item];
  const next = list.slice();
  next[index] = item;
  return next;
}

/** How many activity rows a list keeps in memory; older ones come back through "load more". */
export const ACTIVITY_LIMIT = 500;

export function putActivity(instanceId: string, item: Activity) {
  const add = (page: ActivityPage): ActivityPage => {
    const items = upsertById(page.items, item, true);
    if (items.length <= ACTIVITY_LIMIT) return { ...page, items };
    // The cursor is the last row's id, so paging picks up right after what's kept.
    const kept = items.slice(0, ACTIVITY_LIMIT);
    return { items: kept, next: kept.at(-1)!.id };
  };
  const isNew = !peek<ActivityPage>(keys.activity(instanceId, 'all'))?.items.some((existing) => existing.id === item.id);
  updateData<ActivityPage>(keys.activity(instanceId, 'all'), add);
  if (item.outcome === 'error') updateData<ActivityPage>(keys.activity(instanceId, 'error'), add);
  if (isNew) {
    const fresh = peek<string[]>(keys.fresh(instanceId)) ?? [];
    setData(keys.fresh(instanceId), [...fresh, item.id].slice(-ACTIVITY_LIMIT));
  }
}

/** Waiting on the owner: pending and not yet past its deadline. */
export function isWaiting(approval: Approval, now = Date.now()): boolean {
  return approval.status === 'pending' && !(Date.parse(approval.expires_at) <= now);
}

/** Pending approvals first, newest first; resolved ones after. */
export function sortApprovals(list: Approval[]): Approval[] {
  return list.toSorted((a, b) => {
    const pa = a.status === 'pending' ? 0 : 1;
    const pb = b.status === 'pending' ? 0 : 1;
    if (pa !== pb) return pa - pb;
    return b.created_at.localeCompare(a.created_at);
  });
}

export function putApproval(approval: Approval) {
  const key = keys.approvals(approval.instance_id);
  const before = peek<Approval[]>(key);
  updateData<Approval[]>(key, (list) => sortApprovals(upsertById(list, approval, true)));
  if (!before) setData(key, [approval]);
  // Keep the badge counts in step until the next instance event lands.
  const pending = (peek<Approval[]>(key) ?? []).filter((item) => isWaiting(item)).length;
  const instance = peek<Instance>(keys.instance(approval.instance_id));
  if (instance && instance.counts.pending_approvals !== pending) {
    putInstance({ ...instance, counts: { ...instance.counts, pending_approvals: pending } });
  } else if (!instance) {
    updateData<Instance[]>(keys.instances, (list) =>
      list.map((item) => (item.id === approval.instance_id ? { ...item, counts: { ...item.counts, pending_approvals: pending } } : item)),
    );
  }
}

// ---- The event streams ------------------------------------------------------

/** One server-sent event stream: where it lives, what it listens for, what to refetch after a gap. */
type Spec = { id: string; url: string; wire: (source: EventSourceLike) => void; recover: () => void };
type Stream = { spec: Spec; refs: number; source: EventSourceLike | null; attempt: number; timer: ReturnType<typeof setTimeout> | null; stopped: boolean };
const streams = new Map<string, Stream>();

/** Installed by the connection layer: every stream just dropped, so check whether the server is still there. */
export const liveHooks = { allDown: () => {} };
let allDown = false;

/** Tell the connection layer once when the last open stream drops. */
function checkAllDown() {
  const down = streams.size > 0 && [...streams.values()].every((stream) => stream.source?.readyState !== 1);
  if (down && !allDown) liveHooks.allDown();
  allDown = down;
}

function parse<T>(event: MessageEvent): T | null {
  try {
    return JSON.parse(String(event.data)) as T;
  } catch {
    return null;
  }
}

function open(stream: Stream) {
  if (stream.stopped) return;
  const { id, url, wire, recover } = stream.spec;
  setData<LiveState>(keys.live(id), stream.attempt === 0 ? 'connecting' : 'reconnecting');
  const source = transport.eventSource(url);
  stream.source = source;

  source.onopen = () => {
    const recovering = stream.attempt > 0;
    stream.attempt = 0;
    allDown = false;
    setData<LiveState>(keys.live(id), 'open');
    // Anything that happened while the stream was down arrives by refetching.
    if (recovering) recover();
  };
  wire(source);
  source.onerror = () => {
    // The browser retries on its own while the stream is CONNECTING; once it
    // gives up (CLOSED, e.g. after a 5xx) the portal takes over with backoff.
    if (source.readyState !== 2 || stream.stopped) {
      setData<LiveState>(keys.live(id), 'reconnecting');
      if (stream.attempt === 0) stream.attempt = 1;
      if (!stream.stopped) checkAllDown();
      return;
    }
    source.close();
    stream.source = null;
    const delay = backoff(stream.attempt);
    stream.attempt += 1;
    setData<LiveState>(keys.live(id), 'reconnecting');
    stream.timer = setTimeout(() => open(stream), delay);
    checkAllDown();
  };
}

function subscribe(spec: Spec): () => void {
  let stream = streams.get(spec.id);
  if (!stream) {
    stream = { spec, refs: 0, source: null, attempt: 0, timer: null, stopped: false };
    streams.set(spec.id, stream);
    open(stream);
  }
  stream.refs += 1;
  const current = stream;
  return () => {
    current.refs -= 1;
    if (current.refs > 0) return;
    current.stopped = true;
    if (current.timer) clearTimeout(current.timer);
    current.source?.close();
    streams.delete(spec.id);
  };
}

function macStream(id: string): Spec {
  return {
    id,
    url: api.eventsUrl(id),
    wire(source) {
      source.addEventListener('instance', (event) => {
        const instance = parse<Instance>(event);
        if (instance) putInstance(instance);
      });
      source.addEventListener('activity', (event) => {
        const item = parse<Activity>(event);
        if (item) putActivity(id, item);
      });
      source.addEventListener('approval', (event) => {
        const approval = parse<Approval>(event);
        if (approval) putApproval(approval);
      });
      source.addEventListener('image', (event) => {
        const image = parse<ImageStatus>(event);
        if (image) setData(keys.images, image);
      });
      // Review lists refetch while someone is looking at them.
      source.addEventListener('review', () => invalidate(`review:${id}:`));
    },
    recover() {
      void fetchKey(keys.instance(id), () => api.instance(id)).then(putInstance).catch(() => {});
      invalidate(`review:${id}:`);
      void fetchKey(keys.approvals(id), () => api.approvals(id)).catch(() => {});
      // Activity lists refetch with their own fetchers (they know the filter).
      invalidate(`activity:${id}:`);
    },
  };
}

/** The admin stream's key in the live-state cache. Mac ids never start with "@". */
export const ADMIN_STREAM = '@admin';

const adminStream: Spec = {
  id: ADMIN_STREAM,
  url: api.adminEventsUrl(),
  wire(source) {
    source.addEventListener('updates', (event) => {
      const status = parse<UpdatesStatus>(event);
      if (status) setData(keys.updates, status);
    });
    source.addEventListener('image', (event) => {
      const image = parse<ImageStatus>(event);
      if (image) setData(keys.images, image);
    });
  },
  recover() {
    void fetchKey(keys.updates, api.updates).catch(() => {});
  },
};

/** Subscribe to a Mac's live events while mounted. Shared across components. */
export function subscribeLive(id: string): () => void {
  return subscribe(macStream(id));
}

export function useLive(id: string | null) {
  useEffect(() => (id ? subscribeLive(id) : undefined), [id]);
}

/** Server-wide events for admins (updates, the image builder) while mounted. */
export function useAdminLive(on: boolean) {
  useEffect(() => (on ? subscribe(adminStream) : undefined), [on]);
}

/** Restart every stream now (after the network comes back). */
export function reconnectAll() {
  for (const stream of streams.values()) {
    if (stream.source) continue;
    if (stream.timer) clearTimeout(stream.timer);
    open(stream);
  }
}
