// Destructive actions wait a few seconds with an Undo button before they run
// (Settings › General › Undo time; 0 runs them at once). Leaving never drops one:
//   - moving around the app: the countdown carries on.
//   - closing or reloading the page: it runs now (its request is keepalive).
//   - switching tabs on a computer: the countdown carries on, and catches up on
//     return if the browser paused timers meanwhile.
//   - leaving the app on a phone or tablet: it runs now, since the system can
//     close a backgrounded page without warning.
import { currentPreferences } from './preferences';
import { toastManager } from './toast';

type Pending = { timer: ReturnType<typeof setTimeout>; due: number; run: () => void };
const pending = new Map<string, Pending>();

function runAll(onlyDue = false) {
  const now = Date.now();
  for (const [id, item] of pending) {
    if (onlyDue && item.due > now) continue;
    clearTimeout(item.timer);
    item.run();
    pending.delete(id);
  }
}

const touch = () => typeof matchMedia !== 'undefined' && matchMedia('(pointer: coarse)').matches;

if (typeof window !== 'undefined') {
  addEventListener('pagehide', () => runAll());
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') runAll(true);
    else if (touch()) runAll();
  });
}

/**
 * Runs `action` after the undo time, showing `title` with an Undo button in
 * the meantime. Returns at once; errors surface through the action itself.
 */
export function withUndo(title: string, action: () => unknown): void {
  const seconds = currentPreferences().undo_seconds;
  if (!seconds) {
    void action();
    return;
  }
  let id = '';
  const run = () => {
    if (!pending.delete(id)) return;
    toastManager.close(id);
    void action();
  };
  id = toastManager.add({
    title,
    type: 'undo',
    timeout: 0,
    data: { seconds },
    actionProps: {
      children: 'Undo',
      onClick: () => {
        const item = pending.get(id);
        if (item) clearTimeout(item.timer);
        pending.delete(id);
        toastManager.close(id);
      },
    },
  });
  pending.set(id, { timer: setTimeout(run, seconds * 1000), due: Date.now() + seconds * 1000, run });
}
