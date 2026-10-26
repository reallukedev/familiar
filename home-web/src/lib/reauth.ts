import { hooks } from './api';
import { createStore } from './store';

/** Why the password is asked for: a sensitive change, or unlocking encrypted Macs. */
export type ReauthReason = 'confirm' | 'unlock';

type Pending = { resolve: (ok: boolean) => void; reason: ReauthReason } | null;

/** The open password confirmation, if any. One at a time; callers share it. */
export const reauthRequest = createStore<Pending>(null);
let shared: Promise<boolean> | null = null;

/**
 * The session ended while a confirmation was asked for or open: close the
 * sheet as cancelled and send the person to sign in.
 */
export function sessionEnded() {
  reauthRequest.get()?.resolve(false);
  hooks.unauthenticated();
}

export function requestReauth(reason: ReauthReason = 'confirm'): Promise<boolean> {
  if (shared) return shared;
  shared = new Promise<boolean>((resolve) => {
    reauthRequest.set({
      reason,
      resolve: (ok) => {
        shared = null;
        reauthRequest.set(null);
        resolve(ok);
      },
    });
  });
  return shared;
}

export function installReauthHook() {
  hooks.reauth = () => requestReauth('confirm');
}
