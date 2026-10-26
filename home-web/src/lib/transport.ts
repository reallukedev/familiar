// The three ways the portal talks to the server. The demo backend (dev only)
// swaps these out; production always uses the browser's own.

export type EventSourceLike = {
  addEventListener(type: string, listener: (event: MessageEvent) => void): void;
  close(): void;
  onerror: ((event: Event) => void) | null;
  onopen: ((event: Event) => void) | null;
  readonly readyState: number;
};

/** What noVNC accepts in place of a URL: anything shaped like a WebSocket. */
export type ScreenChannel = string | object;

export type Transport = {
  fetch: (input: string, init: RequestInit) => Promise<Response>;
  eventSource: (url: string) => EventSourceLike;
  screen: (instanceId: string) => ScreenChannel;
};

export function screenUrl(instanceId: string, origin = location.origin): string {
  const url = new URL(`/api/instances/${encodeURIComponent(instanceId)}/screen`, origin);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.href;
}

export const transport: Transport = {
  fetch: (input, init) => fetch(input, init),
  eventSource: (url) => new EventSource(url, { withCredentials: true }),
  screen: (instanceId) => screenUrl(instanceId),
};
