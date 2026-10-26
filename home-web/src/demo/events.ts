// A stand-in for EventSource that the demo server pushes into (development only).
import type { EventSourceLike } from '../lib/transport';

type Listener = (event: MessageEvent) => void;
const open = new Map<string, Set<FakeEventSource>>();

export class FakeEventSource implements EventSourceLike {
  readyState = 0;
  onerror: ((event: Event) => void) | null = null;
  onopen: ((event: Event) => void) | null = null;
  readonly url: string;
  readonly #listeners = new Map<string, Set<Listener>>();
  readonly #instance: string;

  constructor(url: string, canOpen: () => boolean) {
    this.url = url;
    // A Mac's stream is keyed by its id; the admin stream by "@admin".
    this.#instance = url.includes('/api/admin/events') ? '@admin' : decodeURIComponent(url.match(/instances\/([^/]+)\/events/)?.[1] ?? '');
    setTimeout(() => {
      if (this.readyState === 2) return;
      if (!canOpen()) {
        this.readyState = 2;
        this.onerror?.(new Event('error'));
        return;
      }
      this.readyState = 1;
      let set = open.get(this.#instance);
      if (!set) open.set(this.#instance, (set = new Set()));
      set.add(this);
      this.onopen?.(new Event('open'));
    }, 60);
  }

  addEventListener(type: string, listener: Listener) {
    let set = this.#listeners.get(type);
    if (!set) this.#listeners.set(type, (set = new Set()));
    set.add(listener);
  }

  close() {
    this.readyState = 2;
    open.get(this.#instance)?.delete(this);
  }

  dispatch(type: string, data: unknown) {
    const event = new MessageEvent(type, { data: JSON.stringify(data) });
    this.#listeners.get(type)?.forEach((listener) => listener(event));
  }

  /** Simulate the server dropping the stream. */
  fail() {
    this.readyState = 2;
    open.get(this.#instance)?.delete(this);
    this.onerror?.(new Event('error'));
  }
}

export function emit(instanceId: string, type: string, data: unknown) {
  open.get(instanceId)?.forEach((source) => source.dispatch(type, data));
}

export function dropAll() {
  for (const set of open.values()) for (const source of [...set]) source.fail();
}
