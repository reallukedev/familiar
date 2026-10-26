// SYNTHETIC demo of per-person encryption: locked Macs after a restart,
// recovery keys, encrypting an older Mac, and reset links that need the
// recovery key. Development and tests only. Every key here is invented.
import type { Instance } from '../lib/types';
import { normalizeRecoveryKey } from '../lib/recoveryKey';
import { emit } from './events';
import type { DemoState } from './state';

type Result = { status: number; body?: unknown; blob?: Blob };
type Ctx = { state: DemoState; query: URLSearchParams; body: Record<string, unknown> };
type On = (method: string, pattern: string, handler: (ctx: Ctx, ...params: string[]) => Result | Promise<Result>) => void;
type Helpers = {
  json: (body: unknown, status?: number) => Result;
  fail: (status: number, code: string, message: string, extra?: Record<string, unknown>, field?: string) => Result;
  mine: (state: DemoState, id: string) => Instance | null;
  sensitive: (state: DemoState) => Result | null;
  update: (state: DemoState, instance: Instance, patch: Partial<Instance>) => void;
};

/**
 * The recovery key the demo's reset link (`/invite/demo-reset`) accepts, with
 * or without dashes, in any case. Synthetic: it unlocks nothing real.
 */
export const DEMO_RECOVERY_KEY = 'DEMO-RCVR-KEY2-3456-ABCD-EFGH-JKLM-NPQR';

/** Encrypted Macs the demo reset link says the account has. */
export const DEMO_RESET_ENCRYPTED_MACS = 2;

const LOCKED_MESSAGE = 'This Mac is locked. Sign in again to unlock it.';

/** A fresh, random-looking recovery key in the server's format: 8 groups of 4 base32 characters. */
export function newRecoveryKey(): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  const chars = Array.from(bytes, (b) => alphabet[b % 32]).join('');
  return chars.match(/.{4}/g)!.join('-');
}

/** Locked: encrypted while the person's key isn't unlocked. Running Macs keep running. */
export function syncLocks(state: DemoState) {
  for (const instance of state.instances) {
    const locked = !!instance.encrypted && state.locked;
    if (instance.locked !== locked) {
      instance.locked = locked;
      emit(instance.id, 'instance', instance);
    }
  }
}

/** Signing in (password plus code) or confirming the password unlocks the key. */
export function unlock(state: DemoState) {
  state.locked = false;
  syncLocks(state);
}

function off(instance: Instance) {
  return instance.state === 'stopped' || instance.state === 'hibernated' || instance.state === 'error';
}

/**
 * What the server answers before an action that needs a locked Mac's key:
 * starting or waking it, snapshots while it's off, or encrypting it.
 */
export function lockedGuard(state: DemoState, method: string, path: string, body: Record<string, unknown>, helpers: Pick<Helpers, 'fail' | 'mine'>): Result | null {
  if (method === 'GET' || !state.locked) return null;
  const match = path.match(/^\/api\/instances\/([^/]+)\/(power|snapshots|encrypt)(\/.*)?$/);
  if (!match) return null;
  const instance = helpers.mine(state, decodeURIComponent(match[1]!));
  if (!instance?.encrypted) return null;
  const kind = match[2];
  if (kind === 'power' && !['start', 'restart', 'wake'].includes(String(body.action))) return null;
  if (kind === 'power' && body.action === 'wake' && instance.state === 'sleeping') return null;
  if (kind === 'snapshots' && !off(instance)) return null;
  if (kind === 'snapshots' && method === 'PUT') return null;
  return helpers.fail(423, 'locked', LOCKED_MESSAGE);
}

const timers = new Map<string, ReturnType<typeof setTimeout>>();

export function registerEncryption(on: On, { json, fail, mine, sensitive, update }: Helpers) {
  on('POST', '/api/account/recovery-key', ({ state }) => {
    const gate = sensitive(state);
    if (gate) return gate;
    if (state.locked) return fail(423, 'locked', 'Your Macs are locked. Confirm your password to unlock them first.');
    state.recoveryKeyNeeded = true;
    return json({ recovery_key: newRecoveryKey() });
  });
  on('POST', '/api/account/recovery-key/saved', ({ state }) => {
    state.recoveryKeyNeeded = false;
    return json({});
  });
  on('POST', '/api/instances/:id/encrypt', ({ state }, id) => {
    const instance = mine(state, id);
    if (!instance) return fail(404, 'not_found', 'That Mac doesn’t exist or isn’t yours.');
    if (instance.driver !== 'qemu' || instance.encrypted || !instance.can_encrypt) return fail(400, 'invalid', 'This Mac can’t be encrypted.');
    if (!off(instance) || instance.state === 'hibernated') return fail(409, 'conflict', 'Shut down this Mac first.');
    const fast = typeof navigator !== 'undefined' && navigator.webdriver;
    const steps = 20;
    const tick = (n: number) => {
      if (n >= steps) {
        state.snapshots[id] = [];
        update(state, instance, { state: 'stopped', state_detail: null, encrypted: true, can_encrypt: false, encrypt_progress: null, locked: false });
        return;
      }
      update(state, instance, { state: 'encrypting', encrypt_progress: n / steps, state_detail: `Encrypting… ${Math.round((n / steps) * 100)}%` });
      timers.set(id, setTimeout(() => tick(n + 1), fast ? 60 : 400));
    };
    clearTimeout(timers.get(id));
    tick(0);
    return json(instance);
  });
}

/** `GET /api/invites/demo-reset` (sam) and `demo-reset-2fa` (jo.park, with two-factor): an account with encrypted Macs. */
export function resetInviteInfo(username: string) {
  return {
    role: 'member',
    expires_at: new Date(Date.now() + 86_400_000).toISOString(),
    reset_username: username,
    needs_recovery_key: true,
    encrypted_macs: DEMO_RESET_ENCRYPTED_MACS,
  };
}

/** Checks the recovery-key step of `POST /api/invites/demo-reset/accept`. Null when it may go ahead. */
export function checkResetKey(body: Record<string, unknown>, fail: Helpers['fail']): Result | null {
  if (body.discard_encrypted_macs === true) return null;
  if (typeof body.recovery_key !== 'string' || !body.recovery_key.trim()) {
    return fail(409, 'recovery_key_required', 'Enter your recovery key to keep your encrypted Macs, or choose to erase them.');
  }
  if (normalizeRecoveryKey(body.recovery_key) !== normalizeRecoveryKey(DEMO_RECOVERY_KEY)) {
    return fail(400, 'invalid', 'That recovery key isn’t right. Check it against the one you saved.', {}, 'recovery_key');
  }
  return null;
}
