// Turns on the demo backend (development only; see main.tsx). Scenarios:
//   ?demo&scenario=one|empty|many|errors|booting|admin|member|firstrun|signedout|offline|longnames|noguest|approvals|enroll
//   Software Update: updates|updates-available|updates-restarting|updates-disabled|updates-nightly
//   Encryption: locked (after a restart; Unlock with the demo password) | recovery (no saved recovery key).
//   "Old Sonoma Mac" in many, locked and recovery is unencrypted: its Settings offer Encrypt This Mac.
//   /invite/demo-reset needs the recovery key DEMO-RCVR-KEY2-3456-ABCD-EFGH-JKLM-NPQR, or Don't Have It? to erase.
//   /invite/demo-reset-2fa is the same for jo.park, who has two-factor: it finishes with code 123456 on the page.
// Demo sign-in: alex or sam, password "demo-password-1", code 123456.
// The setup link is /setup?token=demo-setup; invites are /invite/demo-join and /invite/demo-reset.
import { uploads } from '../lib/api';
import { transport } from '../lib/transport';
import { syncLocks } from './encryption';
import { dropAll, FakeEventSource } from './events';
import { boot, handle, newActivity, newApproval, runInstall } from './server';
import { createState, SCENARIOS, type DemoState, type Scenario } from './state';

declare global {
  interface Window {
    __demo?: {
      state: DemoState;
      setOffline: (offline: boolean) => void;
      activity: (id: string, outcome?: 'ok' | 'error') => void;
      approval: (id: string) => void;
      dropStreams: () => void;
      expireConfirmation: () => void;
      /** As if Familiar restarted: encrypted Macs lock until the password is entered again. */
      lockKeys: () => void;
    };
  }
}

function remember(key: string, value: string | null) {
  try {
    if (value === null) sessionStorage.removeItem(key);
    else sessionStorage.setItem(key, value);
  } catch {
    // Storage blocked; the URL still works.
  }
}
function recall(key: string): string | null {
  try {
    return sessionStorage.getItem(key);
  } catch {
    return null;
  }
}

export function installDemo(params: URLSearchParams) {
  const asked = params.get('scenario') as Scenario | null;
  const scenario: Scenario = asked && SCENARIOS.includes(asked) ? asked : ((recall('dot-home:scenario') as Scenario | null) ?? 'one');
  remember('dot-home:demo', '1');
  remember('dot-home:scenario', scenario);
  const state = createState(scenario, navigator.userAgent);
  const automated = navigator.webdriver;
  document.documentElement.dataset.demo = scenario;

  transport.fetch = async (input, init) => {
    const url = new URL(input, location.origin);
    if (!url.pathname.startsWith('/api/')) return fetch(input, init);
    await new Promise((resolve) => setTimeout(resolve, automated ? 30 : 120 + Math.random() * 180));
    if (state.offline || Date.now() < state.restartingUntil) throw new TypeError('Failed to fetch');
    // Uploads send a Blob; the demo only needs its size.
    const body = typeof init.body === 'string' && init.body ? (JSON.parse(init.body) as Record<string, unknown>) : init.body instanceof Blob ? { __size: init.body.size } : {};
    const result = await handle(state, (init.method ?? 'GET').toUpperCase(), url, body, new Headers(init.headers));
    if (result.blob) return new Response(result.blob, { status: result.status, headers: { 'Content-Type': result.blob.type || 'image/jpeg' } });
    if (result.status === 204) return new Response(null, { status: 204 });
    return new Response(JSON.stringify(result.body ?? {}), { status: result.status, headers: { 'Content-Type': 'application/json' } });
  };
  // Uploads go through XMLHttpRequest for progress; the demo plays the bytes
  // going out at a home-broadband pace, then answers through the fetch shim.
  uploads.send = (path, file, headers, { signal, onProgress }) =>
    new Promise<void>((resolve, reject) => {
      const aborted = () => new DOMException('The upload was cancelled.', 'AbortError');
      if (signal?.aborted) return reject(aborted());
      const duration = automated ? 60 : Math.min(9000, Math.max(800, (file.size / (40 * 1024 * 1024)) * 1000));
      const started = performance.now();
      const timer = setInterval(() => {
        if (state.offline) {
          stop();
          return reject(new TypeError('Failed to fetch'));
        }
        const fraction = Math.min(1, (performance.now() - started) / duration);
        onProgress?.(fraction);
        if (fraction >= 1) {
          stop();
          resolve();
        }
      }, 50);
      const onAbort = () => {
        stop();
        reject(aborted());
      };
      const stop = () => {
        clearInterval(timer);
        signal?.removeEventListener('abort', onAbort);
      };
      signal?.addEventListener('abort', onAbort, { once: true });
      onProgress?.(0);
    }).then(() => transport.fetch(path, { method: 'POST', headers, body: file, credentials: 'same-origin', cache: 'no-store', signal }));
  transport.eventSource = (url) => new FakeEventSource(url, () => !state.offline && Date.now() >= state.restartingUntil);
  transport.screen = (id) => {
    const instance = state.instances.find((item) => item.id === id);
    // Loaded lazily so the canvas code stays out of the main demo chunk.
    return new LazyChannel(id, instance?.name ?? 'Mac');
  };

  window.__demo = {
    state,
    setOffline: (offline) => {
      state.offline = offline;
      if (offline) dropAll();
    },
    activity: (id, outcome = 'ok') => void newActivity(state, id, outcome),
    approval: (id) => void newApproval(state, id),
    dropStreams: () => dropAll(),
    expireConfirmation: () => {
      state.confirmedUntil = 0;
    },
    lockKeys: () => {
      state.locked = true;
      state.confirmedUntil = 0;
      syncLocks(state);
    },
  };

  // Walks the whole update: downloading, installing, restarting, then back on the new version.
  if (scenario === 'updates-restarting') setTimeout(() => {
    if (state.updates.available) runInstall(state, state.updates.available, 'installed');
  }, 900);

  if (scenario === 'booting') setTimeout(() => {
    const mac = state.instances[0];
    if (mac) boot(state, mac, automated);
  }, 600);

  // A little life for people clicking around; tests drive events themselves.
  if (!automated) {
    setInterval(() => {
      const ready = state.instances.filter((item) => item.state === 'ready' && !item.paused);
      const pick = ready[Math.floor(Math.random() * ready.length)];
      if (pick) newActivity(state, pick.id, Math.random() < 0.1 ? 'error' : 'ok');
    }, 9000);
    if (scenario === 'one') setTimeout(() => state.instances[0] && newApproval(state, state.instances[0].id), 20_000);
  }
}

/** Defers the RFB stand-in's canvas code until a screen is actually opened. */
class LazyChannel {
  binaryType = 'arraybuffer';
  protocol = 'binary';
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  private inner: { send(data: ArrayBuffer | Uint8Array): void; close(): void } | null = null;
  private queue: (ArrayBuffer | Uint8Array)[] = [];

  constructor(id: string, name: string) {
    void import('./screen').then(({ FakeScreenChannel }) => {
      const channel = new FakeScreenChannel(id, name);
      channel.onopen = (event) => {
        this.readyState = 1;
        this.onopen?.(event);
        this.queue.forEach((item) => channel.send(item));
        this.queue = [];
      };
      channel.onmessage = (event) => this.onmessage?.(event);
      channel.onclose = (event) => {
        this.readyState = 3;
        this.onclose?.(event);
      };
      this.inner = channel;
    });
  }

  send(data: ArrayBuffer | Uint8Array) {
    if (this.inner && this.readyState === 1) this.inner.send(data);
    else this.queue.push(data);
  }

  close() {
    this.readyState = 3;
    this.inner?.close();
  }
}
