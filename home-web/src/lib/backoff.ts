/**
 * Reconnect delay for attempt `n` (0-based): 1 s doubling to a 30 s ceiling,
 * spread by up to ±20% so many tabs don't reconnect in lockstep.
 */
export function backoff(attempt: number, random: () => number = Math.random, baseMs = 1000, maxMs = 30_000): number {
  const exp = Math.min(maxMs, baseMs * 2 ** Math.max(0, attempt));
  const jitter = 1 + (random() * 0.4 - 0.2);
  return Math.round(Math.min(maxMs, exp * jitter));
}
