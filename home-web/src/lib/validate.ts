// Client-side checks that mirror the server's rules (docs/home/API.md), so
// people learn about a mistake before they submit. The server still decides.

const USERNAME = /^[a-z0-9._-]{2,32}$/;

export function usernameError(value: string): string | null {
  if (!value) return 'Choose a username.';
  if (value.length < 2 || value.length > 32) return 'Use 2 to 32 characters.';
  if (!USERNAME.test(value)) return 'Use lowercase letters, numbers, dots, dashes and underscores.';
  return null;
}

export function passwordError(value: string): string | null {
  if (value.length < 10) return 'Use at least 10 characters.';
  if (value.length > 256) return 'Use 256 characters or fewer.';
  return null;
}

export function sha256Error(value: string): string | null {
  if (!value) return null;
  return /^[a-f0-9]{64}$/i.test(value.trim()) ? null : 'A SHA-256 checksum is 64 hexadecimal characters.';
}

export function downloadUrlError(value: string): string | null {
  if (!value) return 'Paste a download link.';
  try {
    const url = new URL(value.trim());
    if (url.protocol !== 'https:') return 'Use an https link.';
    if (!/\.(dmg|zip|pkg)$/i.test(url.pathname)) return 'Link to a .dmg, .zip or .pkg file.';
    return null;
  } catch {
    return 'That doesn’t look like a link.';
  }
}

export function caskError(value: string): string | null {
  if (!value) return 'Type a Homebrew cask name, for example visual-studio-code.';
  return /^[a-z0-9][a-z0-9@._+-]*$/.test(value.trim()) ? null : 'Cask names use lowercase letters, numbers and dashes.';
}

export function appStoreIdError(value: string): string | null {
  const id = value.trim().replace(/^id/, '');
  if (!id) return 'Paste the App Store link or its number.';
  return /^\d{6,12}$/.test(id) ? null : 'Use the number from the App Store link, for example 497799835.';
}

/** Accepts "497799835", "id497799835" or a full apps.apple.com link. */
export function appStoreId(value: string): string {
  const match = value.match(/id(\d{6,12})/) ?? value.match(/^(\d{6,12})$/);
  return match?.[1] ?? value.trim();
}
