const relative = new Intl.RelativeTimeFormat('en', { numeric: 'auto', style: 'long' });
const clock = new Intl.DateTimeFormat('en', { hour: 'numeric', minute: '2-digit' });
const dayLong = new Intl.DateTimeFormat('en', { weekday: 'long', month: 'long', day: 'numeric' });
const dateShort = new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', year: 'numeric' });
const dateTime = new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });

const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();

/** "Just now", "4 minutes ago", "3:41 PM", "Yesterday", "Oct 8, 2026". */
export function when(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return '';
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return '';
  const seconds = Math.round((now - at.getTime()) / 1000);
  if (seconds < 45 && seconds > -45) return 'Just now';
  if (seconds > 0 && seconds < 3600) return relative.format(-Math.max(1, Math.round(seconds / 60)), 'minute');
  if (sameDay(at, new Date(now))) return clock.format(at);
  if (sameDay(at, new Date(now - 86_400_000))) return 'Yesterday';
  return dateShort.format(at);
}

/** Absolute time for tooltips and accessible labels. */
export function stamp(iso: string | null | undefined): string {
  if (!iso) return '';
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? '' : dateTime.format(at);
}

export function day(iso: string | null | undefined): string {
  if (!iso) return '';
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? '' : dateShort.format(at);
}

/** Section heading for a run of activity on one day. */
export function dayHeading(iso: string, now = Date.now()): string {
  const at = new Date(iso);
  if (sameDay(at, new Date(now))) return 'Today';
  if (sameDay(at, new Date(now - 86_400_000))) return 'Yesterday';
  return dayLong.format(at);
}

/** "in 42 seconds", "in 3 minutes", "Expired". */
export function expiresIn(iso: string, now = Date.now()): string {
  const seconds = Math.round((new Date(iso).getTime() - now) / 1000);
  if (seconds <= 0) return 'Expired';
  if (seconds < 60) return `${seconds} s left`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min left`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h left`;
  return `${Math.round(hours / 24)} days left`;
}

export function duration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

/** "3 days, 4 hours", "12 minutes", "under a minute". */
export function uptime(seconds: number): string {
  if (seconds < 60) return 'under a minute';
  const d = Math.floor(seconds / 86_400);
  const h = Math.floor((seconds % 86_400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? '' : 's'}`;
  if (d) return h ? `${plural(d, 'day')}, ${plural(h, 'hour')}` : plural(d, 'day');
  if (h) return m ? `${plural(h, 'hour')}, ${plural(m, 'minute')}` : plural(h, 'hour');
  return plural(m, 'minute');
}

const UNITS = ['bytes', 'KB', 'MB', 'GB', 'TB'];
/** Decimal units, as macOS shows storage. */
export function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value)) return '';
  let n = value;
  let unit = 0;
  while (n >= 1000 && unit < UNITS.length - 1) {
    n /= 1000;
    unit += 1;
  }
  if (unit === 0) return n === 1 ? '1 byte' : `${n} bytes`;
  return `${n >= 100 ? Math.round(n) : n.toFixed(1).replace(/\.0$/, '')} ${UNITS[unit]}`;
}

/** Memory in the units people buy it in. */
export function memory(mb: number): string {
  if (mb < 1024) return `${mb} MB`;
  const gb = mb / 1024;
  return `${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`;
}

/** "Never", "15 minutes", "2 hours". */
export function minutes(value: number): string {
  if (value <= 0) return 'Never';
  if (value < 60) return `${value} minute${value === 1 ? '' : 's'}`;
  const h = value / 60;
  return Number.isInteger(h) ? `${h} hour${h === 1 ? '' : 's'}` : `${value} minutes`;
}

export function count(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString('en')} ${n === 1 ? one : many}`;
}

/** Initials for an avatar: "Alex Rivera" becomes "AR". */
export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (!parts.length) return '?';
  const first = parts[0]!.charAt(0);
  const last = parts.length > 1 ? parts.at(-1)!.charAt(0) : '';
  return (first + last).toUpperCase();
}

/** A TOTP secret in groups of four, easier to type from a phone. */
export function groupSecret(secret: string): string {
  return secret.replace(/\s+/g, '').replace(/(.{4})(?=.)/g, '$1 ');
}

/**
 * `when` for the middle of a sentence: "just now", "4 minutes ago",
 * "at 3:41 PM", "yesterday", "on Oct 8, 2026".
 */
export function whenInline(iso: string | null | undefined, now = Date.now()): string {
  const text = when(iso, now);
  if (!text) return '';
  if (text === 'Just now' || text === 'Yesterday') return text.toLowerCase();
  if (/ago$/.test(text)) return text;
  return /^\d/.test(text) ? `at ${text}` : `on ${text}`;
}

/** "0:42" or "12:05": time left on a lockout. */
export function countdown(seconds: number): string {
  const s = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}
