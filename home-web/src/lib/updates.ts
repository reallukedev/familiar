// Software update logic: what the status card should say, and how the portal
// finds the server again after it restarts. Pure where it can be, so it is unit tested.
import { createStore } from './store';
import type { AgentChannel, Channel, InstanceAgent, UpdatesStatus } from './types';

export const CHANNEL_NAME: Record<Channel | 'off', string> = { stable: 'Stable', nightly: 'Nightly', off: 'Off' };

export const CHANNEL_DETAIL: Record<Channel | 'off', string> = {
  stable: 'Tested releases only.',
  nightly: 'The newest changes from every commit. Can break.',
  off: 'Never checks for updates.',
};

/** The first seven characters of a commit, as git shows it. */
export function shortCommit(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 7) : '';
}

/** "3.0.0", or "3.2.0-nightly.20261017 (c41de09)" on nightly, where the commit is what people compare. */
export function versionLabel(version: string, commit: string | null | undefined, channel: Channel | null | undefined): string {
  const short = shortCommit(commit);
  if (channel !== 'nightly' || !short || version.includes(short)) return version;
  return `${version} (${short})`;
}

const hourFormat = (hour: number) => {
  const h = ((hour % 24) + 24) % 24;
  return { clock: `${h % 12 === 0 ? 12 : h % 12}:00`, meridiem: h < 12 ? 'AM' : 'PM' };
};

/** "3:00 AM". */
export function hourLabel(hour: number): string {
  const { clock, meridiem } = hourFormat(hour);
  return `${clock} ${meridiem}`;
}

function span(startHour: number, joiner: string): string {
  const start = hourFormat(startHour);
  const end = hourFormat(startHour + 2);
  return start.meridiem === end.meridiem
    ? `${start.clock} ${joiner} ${end.clock} ${end.meridiem}`
    : `${start.clock} ${start.meridiem} ${joiner} ${end.clock} ${end.meridiem}`;
}

/** The two-hour install window in a sentence: "between 3:00 and 5:00 AM", "between 11:00 PM and 1:00 AM". */
export function windowLabel(startHour: number): string {
  return `between ${span(startHour, 'and')}`;
}

/** The same window as a menu item: "3:00 to 5:00 AM". */
export function windowRange(startHour: number): string {
  return span(startHour, 'to');
}

export type NoteLine = { text: string; heading: boolean };

/**
 * Release notes as plain lines. Notes are written in Markdown on the release
 * page; here they are shown as text: heading marks go (the line is kept as a
 * heading), list marks become bullets, and runs of blank lines collapse to one.
 * Nothing is ever parsed as HTML.
 */
export function noteLines(notes: string | null | undefined): NoteLine[] {
  if (!notes) return [];
  const lines: NoteLine[] = [];
  for (const raw of notes.replace(/\r\n?/g, '\n').split('\n')) {
    const line = raw.replace(/\s+$/, '');
    const heading = /^#{1,6}\s+/.test(line);
    const text = line.replace(/^#{1,6}\s+/, '').replace(/^(\s*)[-*+]\s+/, '$1• ');
    if (!text && (!lines.length || !lines.at(-1)!.text)) continue;
    lines.push({ text, heading });
  }
  while (lines.length && !lines.at(-1)!.text) lines.pop();
  return lines;
}

export type Offer =
  /** Updates are off, or nothing has been found on the channel yet. */
  | { kind: 'none' }
  | { kind: 'current' }
  | { kind: 'newer'; version: string }
  /** The channel's latest is older than what runs (after leaving nightly). */
  | { kind: 'switch'; version: string; channel: Channel };

export function offer(status: UpdatesStatus): Offer {
  const available = status.available;
  if (!status.enabled || status.settings.channel === 'off' || !available) return { kind: 'none' };
  if (available.newer) return { kind: 'newer', version: available.version };
  if (available.version === status.current.version) return { kind: 'current' };
  return { kind: 'switch', version: available.version, channel: status.settings.channel };
}

/** What the agent channel menu calls each choice. The default names the channel it resolves to, when known. */
export function agentChannelLabel(channel: AgentChannel, agent: InstanceAgent): string {
  if (channel !== 'default') return CHANNEL_NAME[channel];
  return agent.channel === 'default' ? `Use Server Setting (${CHANNEL_NAME[agent.effective_channel]})` : 'Use Server Setting';
}

// ---- Reconnecting after a restart --------------------------------------------

type PollOptions<T> = {
  /** Asks the server something cheap; rejects while it's down. */
  probe: () => Promise<T>;
  versionOf: (answer: T) => string;
  /** The version that was running when the restart began. */
  from: string;
  intervalMs?: number;
  /** An answer from the old version counts only after the server was down once, or after this long. */
  graceMs?: number;
  /** Called once when the wait passes this, so the page can say it's slow. */
  slowAfterMs?: number;
  onSlow?: () => void;
  signal?: AbortSignal;
};

/**
 * Poll until the server answers again after a restart. The old process can
 * still answer for a moment before it exits, so an answer only counts once the
 * version changed, the server was unreachable at least once, or `graceMs` passed.
 */
export function pollUntilBack<T>({ probe, versionOf, from, intervalMs = 1500, graceMs = 15_000, slowAfterMs = 90_000, onSlow, signal }: PollOptions<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const started = Date.now();
    let wentAway = false;
    let slow = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const stop = () => {
      if (timer) clearTimeout(timer);
      reject(new DOMException('Stopped waiting for the server', 'AbortError'));
    };
    if (signal?.aborted) return stop();
    signal?.addEventListener('abort', stop, { once: true });

    const tick = async () => {
      timer = null;
      const elapsed = Date.now() - started;
      if (!slow && elapsed >= slowAfterMs) {
        slow = true;
        onSlow?.();
      }
      try {
        const answer = await probe();
        if (signal?.aborted) return;
        if (versionOf(answer) !== from || wentAway || Date.now() - started >= graceMs) {
          signal?.removeEventListener('abort', stop);
          resolve(answer);
          return;
        }
      } catch {
        if (signal?.aborted) return;
        wentAway = true;
      }
      timer = setTimeout(() => void tick(), intervalMs);
    };
    timer = setTimeout(() => void tick(), intervalMs);
  });
}

export type RestartState =
  | { phase: 'idle' }
  | { phase: 'waiting'; from: string; slow: boolean };

/** Set while the server restarts for an update. The offline banner stays quiet meanwhile. */
export const restart = createStore<RestartState>({ phase: 'idle' });
