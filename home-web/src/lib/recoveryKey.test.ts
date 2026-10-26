import { describe, expect, it } from 'vitest';
import { formatRecoveryKey, normalizeRecoveryKey, recoveryKeyError, recoveryKeyFile } from './recoveryKey';

const KEY = 'ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567';

describe('recovery keys as people type them', () => {
  it('accepts any case, with or without dashes or spaces', () => {
    expect(normalizeRecoveryKey('abcd efgh-ijkl–mnop qrst-uvwx yz23 4567')).toBe('ABCDEFGHIJKLMNOPQRSTUVWXYZ234567');
    expect(formatRecoveryKey('abcdefghijklmnopqrstuvwxyz234567')).toBe(KEY);
    expect(formatRecoveryKey(KEY)).toBe(KEY);
    expect(recoveryKeyError(KEY.toLowerCase().replace(/-/g, ' '))).toBeNull();
  });

  it('explains what is wrong before asking the server', () => {
    expect(recoveryKeyError('')).toBe('Enter your recovery key.');
    expect(recoveryKeyError('ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4568')).toMatch(/A to Z and the digits 2 to 7/);
    expect(recoveryKeyError('ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23')).toMatch(/32 characters/);
  });

  it('saves a file with the grouped key, the account and the server', () => {
    const text = recoveryKeyFile(KEY.replace(/-/g, ''), 'alex', 'https://home.example.test');
    expect(text).toContain(KEY);
    expect(text).toContain('Account: alex');
    expect(text).toContain('Server: https://home.example.test');
  });
});
