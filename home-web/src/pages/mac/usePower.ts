import { useOptimistic, useState, useTransition } from 'react';
import { api } from '../../lib/api';
import { putInstance } from '../../lib/live';
import { notify } from '../../lib/toast';
import { withUnlock } from '../../lib/unlock';
import { skipsConfirm } from '../../components/Confirm';
import type { Instance, PowerAction } from '../../lib/types';

const DONE: Partial<Record<PowerAction, string>> = {
  start: 'Starting',
  wake: 'Waking',
  stop: 'Shutting down',
  force_stop: 'Shut down',
  restart: 'Restarting',
  sleep: 'Going to sleep',
};

/** Actions that can lose unsaved work on the Mac, so they warn first. Force shut down always does. */
export const WARNS: Partial<Record<PowerAction, { remember?: string }>> = {
  stop: { remember: 'power-stop' },
  restart: { remember: 'power-restart' },
  force_stop: {},
};

/** Power actions, with a warning before the ones that can lose work. */
export function usePower(instance: Instance) {
  const [busy, setBusy] = useState<PowerAction | null>(null);
  const [confirming, setConfirming] = useState<PowerAction | null>(null);

  async function perform(action: PowerAction) {
    setBusy(action);
    try {
      // A locked Mac asks for the password to unlock, then tries again.
      putInstance(await withUnlock(() => api.power(instance.id, action)));
      const done = DONE[action];
      if (done) notify.info(`${instance.name}: ${done}`);
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  function run(action: PowerAction) {
    const warn = WARNS[action];
    if (warn && !(warn.remember && skipsConfirm(warn.remember))) setConfirming(action);
    else void perform(action);
  }

  return { busy, run, confirming, setConfirming, perform };
}

/** The pause switch, flipped at once and settled by the server. */
export function usePause(instance: Instance) {
  const [paused, setOptimistic] = useOptimistic(instance.paused);
  const [pending, startTransition] = useTransition();
  const set = (value: boolean) =>
    startTransition(async () => {
      setOptimistic(value);
      try {
        putInstance(await api.updateInstance(instance.id, { paused: value }));
        notify.success(value ? 'Assistant paused' : 'Assistant resumed');
      } catch (error) {
        notify.error(error);
      }
    });
  return { paused, pending, set };
}
