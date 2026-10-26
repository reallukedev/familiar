import { api } from './api';
import { fetchKey, keys, setData } from './cache';
import { notify } from './toast';
import { pollUntilBack, restart } from './updates';

/**
 * The server is restarting into another version. Wait for it calmly, then
 * pick up the new session and update status. Safe to call more than once.
 */
export function watchRestart(from: string) {
  if (restart.get().phase === 'waiting') return;
  restart.set({ phase: 'waiting', from, slow: false });
  void pollUntilBack({
    probe: api.session,
    versionOf: (session) => session.version,
    from,
    onSlow: () => restart.set((prev) => (prev.phase === 'waiting' ? { ...prev, slow: true } : prev)),
  }).then(async (session) => {
    setData(keys.session, session);
    await fetchKey(keys.updates, api.updates).catch(() => {});
    restart.set({ phase: 'idle' });
    notify.success(`Familiar ${session.version} is running`);
  });
}
