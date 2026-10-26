import { useEffect, useMemo, type CSSProperties, type ReactNode } from 'react';
import { CountBadge } from '../components/Bits';
import { api } from '../lib/api';
import { keys, useQuery } from '../lib/cache';
import { useReducedMotion } from '../lib/platform';
import { onLinkClick, setNavigationAnimator, useLocation } from '../lib/router';
import { useSession } from '../lib/session';
import { ShellContext } from './context';
import styles from './PhoneShell.module.css';

const SHELL = { kind: 'phone' as const, sidebarOpen: false, toggleSidebar: () => {}, sidebarOverlay: false };

/**
 * The iOS idiom: a tab bar for the top-level places, push navigation with the
 * iOS slide for everything below them, and sheets that rise from the bottom.
 */
export function PhoneShell({ children }: { children: ReactNode }) {
  const reduced = useReducedMotion();
  const { route } = useLocation();

  useEffect(() => {
    setNavigationAnimator((direction, update) => {
      const doc = document as Document & { startViewTransition?: (cb: () => void) => { finished: Promise<void> } };
      if (reduced || !doc.startViewTransition || direction === 'none') {
        update();
        return;
      }
      document.documentElement.dataset.nav = direction;
      const transition = doc.startViewTransition(update);
      void transition.finished.finally(() => delete document.documentElement.dataset.nav);
    });
    return () => setNavigationAnimator(null);
  }, [reduced]);

  const immersive = route.name === 'mac' && route.section === 'screen';
  return (
    <ShellContext value={SHELL}>
      <div className={styles.phone}>
        <main id="content">{children}</main>
        {immersive ? null : <TabBar />}
      </div>
    </ShellContext>
  );
}

type Tab = { id: string; label: string; href: string; icon: ReactNode; badge?: number };

function TabBar() {
  const { route } = useLocation();
  const { data: session } = useSession();
  const instances = useQuery(keys.instances, api.instances, { pollMs: 30_000 });
  const pending = (instances.data ?? []).reduce((sum, instance) => sum + instance.counts.pending_approvals, 0);

  const tabs = useMemo(() => {
    const list: Tab[] = [
      { id: 'macs', label: 'Macs', href: '/', icon: <MacsGlyph />, badge: pending },
      { id: 'activity', label: 'Activity', href: '/activity', icon: <ActivityGlyph /> },
      { id: 'account', label: 'Account', href: '/account', icon: <AccountGlyph /> },
    ];
    if (session?.user?.role === 'admin') list.push({ id: 'admin', label: 'Server', href: '/admin', icon: <ServerGlyph /> });
    return list;
  }, [pending, session?.user?.role]);

  const current = route.name === 'activity' ? 'activity' : route.name === 'account' ? 'account' : route.name === 'admin' ? 'admin' : 'macs';

  const index = Math.max(0, tabs.findIndex((tab) => tab.id === current));

  return (
    <nav className={styles.tabbar} aria-label="Tabs">
      <ul className={styles.tabs} style={{ gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))`, '--count': tabs.length, '--index': index } as CSSProperties}>
        <li className={styles.lens} aria-hidden="true" />
        {tabs.map((tab) => (
          <li key={tab.id}>
            <a
              href={tab.href}
              className={`${styles.tab} ${current === tab.id ? styles.current : ''}`}
              aria-current={current === tab.id ? 'page' : undefined}
              onClick={(event) => onLinkClick(event, { direction: 'none' })}
            >
              <span className={styles.icon}>
                {tab.icon}
                {tab.badge ? (
                  <span className={styles.badge}>
                    <CountBadge tone="red" count={tab.badge} label={`${tab.badge} waiting for approval`} />
                  </span>
                ) : null}
              </span>
              <span className={styles.label}>{tab.label}</span>
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );
}

// Tab glyphs: filled shapes, as tab bars use, drawn on a 28 grid.
function MacsGlyph() {
  return (
    <svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true" fill="currentColor">
      <path d="M5.5 5h17A2.5 2.5 0 0 1 25 7.5v10a2.5 2.5 0 0 1-2.5 2.5h-17A2.5 2.5 0 0 1 3 17.5v-10A2.5 2.5 0 0 1 5.5 5Zm.5 2.2a.8.8 0 0 0-.8.8v9a.8.8 0 0 0 .8.8h16a.8.8 0 0 0 .8-.8V8a.8.8 0 0 0-.8-.8H6ZM10 22.2h8a1 1 0 1 1 0 2h-8a1 1 0 1 1 0-2Zm2.8-2.4h2.4v2.6h-2.4z" />
    </svg>
  );
}
function ActivityGlyph() {
  return (
    <svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true" fill="currentColor">
      <path d="M10 7.2h13a1.3 1.3 0 0 1 0 2.6H10a1.3 1.3 0 0 1 0-2.6Zm0 5.5h13a1.3 1.3 0 0 1 0 2.6H10a1.3 1.3 0 0 1 0-2.6Zm0 5.5h13a1.3 1.3 0 0 1 0 2.6H10a1.3 1.3 0 0 1 0-2.6ZM5.3 6.7a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6Zm0 5.5a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6Zm0 5.5a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6Z" />
    </svg>
  );
}
function AccountGlyph() {
  return (
    <svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true" fill="currentColor">
      <path d="M14 3a11 11 0 1 1 0 22 11 11 0 0 1 0-22Zm0 14.6c-3 0-5.5 1.4-6.7 3.5a8.9 8.9 0 0 0 13.4 0c-1.2-2.1-3.7-3.5-6.7-3.5Zm0-10a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z" />
    </svg>
  );
}
function ServerGlyph() {
  return (
    <svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true" fill="currentColor">
      <path d="M6 4.5h16A2 2 0 0 1 24 6.5v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2Zm2.5 3.2a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6ZM6 15h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2Zm2.5 3.2a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6Z" />
    </svg>
  );
}
