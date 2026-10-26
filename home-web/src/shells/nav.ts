import type { IconName } from '../components/Icon';
import type { AdminSection, MacSection } from '../lib/routes';
import type { Instance } from '../lib/types';

export const MAC_NAV: { section: MacSection; label: string; icon: IconName }[] = [
  { section: 'overview', label: 'Overview', icon: 'display' },
  { section: 'screen', label: 'Screen', icon: 'cursor' },
  { section: 'files', label: 'Files', icon: 'folder' },
  { section: 'activity', label: 'Activity', icon: 'list' },
  { section: 'review', label: 'Review', icon: 'check' },
  { section: 'apps', label: 'Apps', icon: 'grid' },
  { section: 'permissions', label: 'Permissions', icon: 'hand' },
  { section: 'connect', label: 'Connections', icon: 'link' },
  { section: 'snapshots', label: 'Snapshots', icon: 'stack' },
  { section: 'settings', label: 'Settings', icon: 'sliders' },
];

export const ADMIN_NAV: { section: AdminSection; label: string; icon: IconName }[] = [
  { section: 'overview', label: 'Server', icon: 'server' },
  { section: 'users', label: 'People', icon: 'people' },
  { section: 'invites', label: 'Invites', icon: 'envelope' },
  { section: 'macs', label: 'All Macs', icon: 'display' },
  { section: 'storage', label: 'Storage Limits', icon: 'disk' },
  { section: 'image', label: 'macOS Image', icon: 'disc' },
  { section: 'updates', label: 'Software Update', icon: 'update' },
  { section: 'settings', label: 'Policies', icon: 'shield' },
];

/** The count a Mac section shows beside its name: approvals waiting, changes to review. */
export function sectionBadge(section: MacSection, counts: Instance['counts']): { count: number; label: string } | null {
  if (section === 'activity') return { count: counts.pending_approvals, label: `${counts.pending_approvals} waiting for approval` };
  if (section === 'review') return { count: counts.pending_review ?? 0, label: `${counts.pending_review ?? 0} to review` };
  return null;
}

export const MAC_TITLES: Record<MacSection, string> = Object.fromEntries(MAC_NAV.map((item) => [item.section, item.label])) as Record<MacSection, string>;

/** The Mac a person was last looking at, so tabs and shortcuts return to it. */
let lastMac: string | null = null;
export function rememberMac(id: string) {
  lastMac = id;
  try {
    sessionStorage.setItem('dot-home:last-mac', id);
  } catch {
    // Private mode: memory only.
  }
}
export function recallMac(): string | null {
  if (lastMac) return lastMac;
  try {
    return sessionStorage.getItem('dot-home:last-mac');
  } catch {
    return null;
  }
}
