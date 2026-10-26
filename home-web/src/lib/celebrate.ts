import { reducedMotion } from './platform';

/**
 * Holds a moment of success on screen before moving on, long enough for
 * Familiar's happy hop to land (780 ms). Shorter with reduced motion, where
 * the expression only cross-fades.
 */
export function celebrate(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, reducedMotion() ? 360 : 820));
}
