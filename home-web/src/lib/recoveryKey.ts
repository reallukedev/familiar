// Recovery keys as people see and type them. Pure, so it is unit tested.

/** Base32 (RFC 4648): A to Z and 2 to 7. */
const ALPHABET = /^[A-Z2-7]+$/;
/** 160 bits: 32 characters, shown as 8 groups of 4. */
export const RECOVERY_KEY_LENGTH = 32;

/** What the person typed, as bare characters: no dashes or spaces, upper case. */
export function normalizeRecoveryKey(text: string): string {
  return text.toUpperCase().replace(/[\s\-–—_.]/g, '');
}

/** Groups of four with dashes, the way the key is shown and saved. */
export function formatRecoveryKey(text: string): string {
  return (normalizeRecoveryKey(text).match(/.{1,4}/g) ?? []).join('-');
}

/** Why a typed key can't be right, or null when it looks like one. The server decides whether it matches. */
export function recoveryKeyError(text: string): string | null {
  const key = normalizeRecoveryKey(text);
  if (!key) return 'Enter your recovery key.';
  if (!ALPHABET.test(key)) return 'Recovery keys use only the letters A to Z and the digits 2 to 7.';
  if (key.length !== RECOVERY_KEY_LENGTH) return `Recovery keys have ${RECOVERY_KEY_LENGTH} characters. Check for a missing group.`;
  return null;
}

/** The text file the Download button saves. */
export function recoveryKeyFile(key: string, account: string | null, origin: string): string {
  return [
    'Familiar Recovery Key',
    '',
    formatRecoveryKey(key),
    '',
    `Account: ${account ?? 'unknown'}`,
    `Server: ${origin}`,
    '',
    'If you forget your password, this key is the only way to keep the data on your encrypted Macs.',
    'Keep it somewhere safe, away from this computer.',
    '',
  ].join('\n');
}
