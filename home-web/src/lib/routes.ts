// Route table. Pure, so it is unit tested and shared by both shells.

export const MAC_SECTIONS = ['screen', 'files', 'activity', 'review', 'apps', 'permissions', 'connect', 'snapshots', 'settings'] as const;
export type MacSection = 'overview' | (typeof MAC_SECTIONS)[number];

export const ADMIN_SECTIONS = ['users', 'invites', 'macs', 'storage', 'image', 'updates', 'settings'] as const;
export type AdminSection = 'overview' | (typeof ADMIN_SECTIONS)[number];

export type Route =
  | { name: 'setup' }
  | { name: 'signin' }
  | { name: 'invite'; code: string }
  | { name: 'macs' }
  | { name: 'mac'; id: string; section: MacSection }
  | { name: 'activity' }
  | { name: 'consent' }
  | { name: 'account' }
  /** The signed-in person's storage across all their Macs. */
  | { name: 'storage' }
  | { name: 'admin'; section: AdminSection }
  /** The image builder's screen. It is the admin's, but never listed with their Macs. */
  | { name: 'builder' }
  | { name: 'missing' };

/** Routes that render without a signed-in session. */
export const PUBLIC_ROUTES: ReadonlySet<Route['name']> = new Set(['setup', 'signin', 'invite']);

function segment(value: string | undefined): string | null {
  if (!value) return null;
  try {
    const decoded = decodeURIComponent(value);
    return decoded && !decoded.includes('/') ? decoded : null;
  } catch {
    return null;
  }
}

export function parseRoute(pathname: string): Route {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
  if (path === '' || path === '/') return { name: 'macs' };
  if (path === '/setup') return { name: 'setup' };
  if (path === '/signin') return { name: 'signin' };
  if (path === '/connect') return { name: 'consent' };
  if (path === '/account') return { name: 'account' };
  if (path === '/activity') return { name: 'activity' };
  if (path === '/storage') return { name: 'storage' };
  if (path === '/admin') return { name: 'admin', section: 'overview' };
  if (path === '/admin/image/screen') return { name: 'builder' };

  const parts = path.split('/').slice(1);
  if (parts[0] === 'invite' && parts.length === 2) {
    const code = segment(parts[1]);
    return code ? { name: 'invite', code } : { name: 'missing' };
  }
  if (parts[0] === 'admin' && parts.length === 2) {
    const section = parts[1] as AdminSection;
    return (ADMIN_SECTIONS as readonly string[]).includes(section) ? { name: 'admin', section } : { name: 'missing' };
  }
  if (parts[0] === 'macs' && (parts.length === 2 || parts.length === 3)) {
    const id = segment(parts[1]);
    if (!id) return { name: 'missing' };
    if (parts.length === 2) return { name: 'mac', id, section: 'overview' };
    const section = parts[2] as MacSection;
    return (MAC_SECTIONS as readonly string[]).includes(section) ? { name: 'mac', id, section } : { name: 'missing' };
  }
  return { name: 'missing' };
}

export function macPath(id: string, section: MacSection = 'overview'): string {
  const base = `/macs/${encodeURIComponent(id)}`;
  return section === 'overview' ? base : `${base}/${section}`;
}

export function adminPath(section: AdminSection = 'overview'): string {
  return section === 'overview' ? '/admin' : `/admin/${section}`;
}

/** Where "back" goes from a route on the phone's navigation stack. */
export function parentPath(route: Route): string | null {
  if (route.name === 'mac') return route.section === 'overview' ? '/' : macPath(route.id);
  if (route.name === 'admin' && route.section !== 'overview') return '/admin';
  if (route.name === 'builder') return '/admin/image';
  return null;
}

/**
 * A `next=` target is honored only when it is a path on this origin. Anything
 * else (another host, a protocol-relative URL, a script URL) falls back to "/".
 */
export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\')) return '/';
  try {
    const url = new URL(next, 'https://portal.invalid');
    if (url.origin !== 'https://portal.invalid') return '/';
    return url.pathname + url.search + url.hash;
  } catch {
    return '/';
  }
}

/** The OAuth server hands back a URL to continue to; only http(s) is followed. */
export function safeRedirect(target: string): string | null {
  try {
    const url = new URL(target, location.origin);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch {
    return null;
  }
}
