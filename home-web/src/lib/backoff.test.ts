import { describe, expect, it } from 'vitest';
import { backoff } from './backoff';

describe('backoff', () => {
  const middle = () => 0.5; // no jitter

  it('doubles from one second and stops at thirty', () => {
    expect([0, 1, 2, 3, 4, 5, 6, 10].map((n) => backoff(n, middle))).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  });

  it('spreads retries by at most 20 percent and never past the ceiling', () => {
    expect(backoff(2, () => 0)).toBe(3200);
    expect(backoff(2, () => 0.999999)).toBe(4800);
    expect(backoff(9, () => 0.999999)).toBe(30000);
  });
});
