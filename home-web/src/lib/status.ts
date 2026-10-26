import type { Instance, InstanceState, PowerAction } from './types';

export type Tone = 'green' | 'orange' | 'red' | 'gray' | 'blue';

type StateInfo = {
  label: string;
  tone: Tone;
  /** Shown when the server sends no detail of its own. */
  fallback: string;
  /** The Mac is on its way somewhere; show progress, not a button. */
  transitional: boolean;
  /** The one action that makes sense from here, if any. */
  primary: { action: PowerAction; label: string } | null;
  /** Where the boot sequence is, 0 to 1, for the progress bar. */
  progress: number | null;
};

const STATES: Record<InstanceState, StateInfo> = {
  stopped: { label: 'Off', tone: 'gray', fallback: 'Starts when you or your assistant need it.', transitional: false, primary: { action: 'start', label: 'Start Mac' }, progress: null },
  starting: { label: 'Starting', tone: 'blue', fallback: 'Turning on the virtual machine.', transitional: true, primary: null, progress: 0.2 },
  booting: { label: 'Starting macOS', tone: 'blue', fallback: 'Waiting for macOS to finish starting.', transitional: true, primary: null, progress: 0.62 },
  ready: { label: 'Ready', tone: 'green', fallback: 'Your assistant can use this Mac.', transitional: false, primary: null, progress: null },
  degraded: { label: 'Not responding', tone: 'orange', fallback: 'The Familiar agent stopped answering. Familiar will restart it if needed.', transitional: false, primary: { action: 'restart', label: 'Restart Mac' }, progress: null },
  sleeping: { label: 'Sleeping', tone: 'gray', fallback: 'Wakes when your assistant needs it.', transitional: false, primary: { action: 'wake', label: 'Wake Mac' }, progress: null },
  hibernated: { label: 'Hibernated', tone: 'gray', fallback: 'Waking takes a few seconds.', transitional: false, primary: { action: 'wake', label: 'Wake Mac' }, progress: null },
  stopping: { label: 'Shutting down', tone: 'gray', fallback: 'Saving work and shutting down.', transitional: true, primary: null, progress: null },
  error: { label: 'Stopped unexpectedly', tone: 'red', fallback: 'It couldn’t restart on its own.', transitional: false, primary: { action: 'start', label: 'Start Mac' }, progress: null },
  encrypting: { label: 'Encrypting', tone: 'blue', fallback: 'Encrypting this Mac’s disk. It can start when that’s done.', transitional: true, primary: null, progress: null },
};

/** How a locked Mac reads everywhere: calm, with one way forward. */
const LOCKED: StateInfo = { label: 'Locked', tone: 'gray', fallback: 'Sign in to unlock.', transitional: false, primary: null, progress: null };

export function stateInfo(state: InstanceState): StateInfo {
  return STATES[state];
}

/**
 * Locked: encrypted, off, and its owner's key isn't unlocked (Familiar
 * restarted). It can't start or wake until they enter their password.
 */
export function isLockedOff(instance: Pick<Instance, 'state' | 'locked'>): boolean {
  return !!instance.locked && (instance.state === 'stopped' || instance.state === 'hibernated' || instance.state === 'error');
}

/** stateInfo, but a locked Mac reads as Locked instead of Off. */
export function instanceInfo(instance: Pick<Instance, 'state' | 'locked'>): StateInfo {
  return isLockedOff(instance) ? LOCKED : STATES[instance.state];
}

export function stateDetail(instance: Pick<Instance, 'state' | 'state_detail' | 'locked' | 'encrypt_progress'>): string {
  if (isLockedOff(instance)) return LOCKED.fallback;
  if (instance.state === 'encrypting' && typeof instance.encrypt_progress === 'number') return `Encrypting… ${Math.round(instance.encrypt_progress * 100)}%`;
  return instance.state_detail?.trim() || STATES[instance.state].fallback;
}

/** Where a boot or an encryption is, 0 to 1, or null when it isn't known. */
export function progressOf(instance: Pick<Instance, 'state' | 'encrypt_progress'>): number | null {
  if (instance.state === 'encrypting') return typeof instance.encrypt_progress === 'number' ? instance.encrypt_progress : null;
  return STATES[instance.state].progress;
}

/** Whether the Mac is running in a way that has a screen to show. */
export function hasScreen(state: InstanceState): boolean {
  return state === 'ready' || state === 'degraded' || state === 'booting';
}

export function isOn(state: InstanceState): boolean {
  return state === 'ready' || state === 'degraded' || state === 'booting' || state === 'starting' || state === 'sleeping';
}

/** Power actions offered in the menu for this state, in menu order. */
export function powerMenu(instance: Pick<Instance, 'state' | 'driver' | 'locked'>): PowerAction[] {
  const vm = instance.driver === 'qemu';
  // Shutting down a locked hibernated Mac still works; nothing else does until it's unlocked.
  if (isLockedOff(instance)) return instance.state === 'hibernated' ? ['stop'] : [];
  switch (instance.state) {
    case 'ready':
    case 'degraded':
      return vm ? ['sleep', 'restart', 'stop', 'force_stop'] : ['restart'];
    case 'booting':
    case 'starting':
      return vm ? ['force_stop'] : [];
    case 'sleeping':
    case 'hibernated':
      return vm ? ['wake', 'stop'] : ['wake'];
    case 'stopping':
      return vm ? ['force_stop'] : [];
    case 'stopped':
    case 'error':
      return ['start'];
    case 'encrypting':
      return [];
  }
}

export const POWER_LABELS: Record<PowerAction, string> = {
  start: 'Start Mac',
  stop: 'Shut Down',
  restart: 'Restart',
  force_stop: 'Force Shut Down',
  sleep: 'Sleep',
  wake: 'Wake Mac',
};

/** Guest permissions the agent needs, each with a plain fix. */
export const GUEST_PERMISSIONS = [
  {
    key: 'accessibility',
    label: 'Accessibility',
    need: 'Click, type and read buttons.',
    fix: 'On the Mac, open System Settings > Privacy & Security > Accessibility and turn on dot-guest.',
  },
  {
    key: 'screen_recording',
    label: 'Screen & System Audio Recording',
    need: 'See the screen.',
    fix: 'On the Mac, open System Settings > Privacy & Security > Screen & System Audio Recording and turn on dot-guest.',
  },
  {
    key: 'full_disk_access',
    label: 'Full Disk Access',
    need: 'Open protected folders like Mail.',
    fix: 'On the Mac, open System Settings > Privacy & Security > Full Disk Access and turn on dot-guest.',
  },
] as const;

export function missingPermissions(instance: Pick<Instance, 'guest'>) {
  const perms = instance.guest?.permissions;
  if (!perms) return [];
  return GUEST_PERMISSIONS.filter((item) => perms[item.key] === false);
}
