// Turns a user agent into the words people use for their devices.

export function deviceName(userAgent: string | null | undefined): string {
  if (!userAgent) return 'Unknown device';
  const ua = userAgent;
  const browser = /Edg\//.test(ua)
    ? 'Edge'
    : /Firefox\//.test(ua)
      ? 'Firefox'
      : /(Chrome|CriOS)\//.test(ua)
        ? 'Chrome'
        : /Safari\//.test(ua)
          ? 'Safari'
          : /curl|python|node|Go-http/i.test(ua)
            ? 'A script'
            : 'A browser';
  const os = /iPhone/.test(ua)
    ? 'iPhone'
    : /iPad/.test(ua)
      ? 'iPad'
      : /Android/.test(ua)
        ? 'Android'
        : /Mac OS X|Macintosh/.test(ua)
          ? 'Mac'
          : /Windows/.test(ua)
            ? 'Windows'
            : /Linux/.test(ua)
              ? 'Linux'
              : '';
  return os ? `${browser} on ${os}` : browser;
}

type SignInLike = { id: string; kind: string; ip: string | null; user_agent: string | null };

/**
 * Sign-ins from a browser and network not seen in any earlier sign-in.
 * `events` is newest first, as the server sends it; the oldest sign-in is
 * the baseline and never counts as new.
 */
export function newDeviceSignIns(events: SignInLike[]): Set<string> {
  const seen = new Set<string>();
  const fresh = new Set<string>();
  const signIns = events.filter((event) => event.kind === 'sign_in').toReversed();
  signIns.forEach((event, index) => {
    const fingerprint = `${deviceName(event.user_agent)}|${event.ip ?? ''}`;
    if (index > 0 && !seen.has(fingerprint)) fresh.add(event.id);
    seen.add(fingerprint);
  });
  return fresh;
}
