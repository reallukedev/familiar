import { ApiError } from './api';
import { invalidate } from './cache';
import { requestReauth } from './reauth';
import { refreshSession } from './session';

/**
 * After a server restart a person's encrypted Macs stay locked until they enter
 * their password again. Confirming it unlocks them; then everything reloads so
 * locked Macs show as off and ready to start. Resolves true once unlocked.
 */
export async function unlockMacs(): Promise<boolean> {
  const ok = await requestReauth('unlock');
  if (!ok) return false;
  await refreshSession().catch(() => {});
  invalidate();
  return true;
}

/** True for the answer an encrypted Mac gives while its owner's key is locked. */
export function isLocked(error: unknown): boolean {
  return error instanceof ApiError && error.code === 'locked';
}

/**
 * Run `work`; if the Mac answers that it's locked, ask for the password to
 * unlock it and try once more.
 */
export async function withUnlock<T>(work: () => Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (error) {
    if (!isLocked(error)) throw error;
    if (!(await unlockMacs())) throw new ApiError('reauth_cancelled', 'Unlock your Macs to continue.', 423);
    return work();
  }
}
