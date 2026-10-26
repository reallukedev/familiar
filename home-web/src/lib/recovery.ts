import { api, hooks } from './api';
import { recoveryKeyFile } from './recoveryKey';
import { createStore } from './store';
import { withUnlock } from './unlock';

/** The recovery key on screen, shown once and never kept anywhere else. */
export const shownRecoveryKey = createStore<string | null>(null);

export function downloadRecoveryKey(key: string, account: string | null, origin: string) {
  const blob = new Blob([recoveryKeyFile(key, account, origin)], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = 'Familiar Recovery Key.txt';
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Make a new key (asking for the password if needed) and show it. The old key stops working. */
export async function showNewRecoveryKey() {
  // Locked after a restart: the password that unlocks also lets the key be rewrapped.
  const { recovery_key } = await withUnlock(api.newRecoveryKey);
  shownRecoveryKey.set(recovery_key);
}

/** Any reply carrying a new recovery key opens the sheet. */
export function installRecoveryHook() {
  hooks.recoveryKey = (key) => shownRecoveryKey.set(key);
}
