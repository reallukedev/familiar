import { api, hooks } from './api';
import { backoff } from './backoff';
import { invalidate } from './cache';
import { liveHooks, reconnectAll } from './live';
import { createStore } from './store';

/** Whether the last request reached the server. Drives the offline banner. */
export const reachability = createStore<{ online: boolean; checking: boolean }>({ online: true, checking: false });

let probeTimer: ReturnType<typeof setTimeout> | null = null;
let probeAttempt = 0;

function scheduleProbe() {
  if (probeTimer) return;
  probeTimer = setTimeout(() => {
    probeTimer = null;
    void checkNow();
  }, backoff(probeAttempt++, Math.random, 2000, 30_000));
}

/** Try the server now (the banner's Try Again, or the browser coming online). */
export async function checkNow() {
  if (probeTimer) {
    clearTimeout(probeTimer);
    probeTimer = null;
  }
  reachability.set((prev) => ({ ...prev, checking: true }));
  try {
    await api.session();
  } catch {
    // reachable(false) has already run and scheduled the next probe.
  } finally {
    reachability.set((prev) => ({ ...prev, checking: false }));
  }
}

export function installConnectionHooks() {
  hooks.reachable = (ok) => {
    const was = reachability.get().online;
    if (ok === was) {
      if (!ok) scheduleProbe();
      return;
    }
    reachability.set((prev) => ({ ...prev, online: ok }));
    if (ok) {
      probeAttempt = 0;
      invalidate();
      reconnectAll();
    } else scheduleProbe();
  };
  // Every live stream dropping at once usually means the server went away;
  // check now instead of waiting for the next request to fail.
  liveHooks.allDown = () => {
    if (reachability.get().online && !reachability.get().checking) void checkNow();
  };
  addEventListener('online', () => void checkNow());
}
