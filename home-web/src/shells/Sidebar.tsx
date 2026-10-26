import { useEffect, useState, type ReactNode } from 'react';
import { Avatar, CountBadge } from '../components/Bits';
import { Icon, type IconName } from '../components/Icon';
import { Tip } from '../components/Tip';
import { api } from '../lib/api';
import { keys, useQuery } from '../lib/cache';
import { isMacKeyboard } from '../lib/platform';
import { openSettings } from '../lib/settingsWindow';
import { onLinkClick, useLocation } from '../lib/router';
import { adminPath, macPath } from '../lib/routes';
import { useSession } from '../lib/session';
import { instanceInfo } from '../lib/status';
import type { Instance } from '../lib/types';
import { NewMacSheet } from '../pages/NewMac';
import { ADMIN_NAV, MAC_NAV, sectionBadge } from './nav';
import styles from './Sidebar.module.css';

export function Sidebar({ onSearch }: { onSearch: () => void }) {
  const { data: session } = useSession();
  const instances = useQuery(keys.instances, api.instances, { pollMs: 20_000 });
  const { route } = useLocation();
  const [creating, setCreating] = useState(false);
  const user = session?.user;
  const currentMac = route.name === 'mac' ? route.id : null;
  const [collapsed, setCollapsed] = useCollapsed();

  return (
    <nav className={styles.sidebar} aria-label="Familiar">
      <div className={styles.scroll}>
      <button type="button" className={styles.search} onClick={onSearch}>
        <Icon name="search" size={14} weight={2} />
        <span>Search</span>
        <kbd className={styles.kbd}>{isMacKeyboard ? '⌘K' : 'Ctrl K'}</kbd>
      </button>

      {user ? (
        <a href="/account" onClick={(event) => onLinkClick(event)} className={`${styles.account} ${route.name === 'account' ? styles.selected : ''}`} aria-current={route.name === 'account' ? 'page' : undefined}>
          <Avatar name={user.display_name} />
          <span className={styles.accountText}>
            <span className={styles.accountName}>{user.display_name}</span>
            <span className={styles.accountSub}>Account and security</span>
          </span>
        </a>
      ) : null}

      <ul className={styles.list}>
        <PlaceItem href="/" icon="house" label="Home" selected={route.name === 'macs'} />
        <PlaceItem href="/storage" icon="disk" label="Storage" selected={route.name === 'storage'} />
      </ul>

      <div className={styles.groupHead}>
        <GroupTitle id="macs" collapsed={collapsed} onToggle={setCollapsed}>
          Macs
        </GroupTitle>
        <Tip label="New Mac">
          <button type="button" className={styles.add} onClick={() => setCreating(true)} aria-label="New Mac">
            <Icon name="plus" size={14} weight={2.2} />
          </button>
        </Tip>
      </div>
      <Fold id="sidebar-macs" open={!collapsed.has('macs')}>
      <ul className={styles.list}>
        {(instances.data ?? []).map((instance) => (
          <MacItem key={instance.id} instance={instance} expanded={instance.id === currentMac} section={route.name === 'mac' ? route.section : null} />
        ))}
        {!instances.data && instances.loading
          ? [0, 1].map((i) => (
              <li key={i} className={styles.placeholder} aria-hidden="true">
                <span />
              </li>
            ))
          : null}
        {instances.data?.length === 0 ? <li className={`${styles.empty} t-footnote`}>No Macs yet.</li> : null}
      </ul>
      </Fold>

      {user?.role === 'admin' ? (
        <>
          <div className={styles.groupHead}>
            <GroupTitle id="server" collapsed={collapsed} onToggle={setCollapsed}>
              Server
            </GroupTitle>
          </div>
          <Fold id="sidebar-server" open={!collapsed.has('server')}>
          <ul className={styles.list}>
            {ADMIN_NAV.map((item) => {
              const href = adminPath(item.section);
              const selected = route.name === 'admin' && route.section === item.section;
              return (
                <li key={item.section}>
                  <a href={href} onClick={(event) => onLinkClick(event)} className={`${styles.item} ${selected ? styles.selected : ''}`} aria-current={selected ? 'page' : undefined}>
                    <span className={styles.itemIcon}>
                      <Icon name={item.icon} size={16} weight={1.8} />
                    </span>
                    <span className={styles.itemLabel}>{item.label}</span>
                  </a>
                </li>
              );
            })}
          </ul>
          </Fold>
        </>
      ) : null}

      </div>
      <div className={styles.footer}>
        <p className={`${styles.version} t-caption`}>{session?.version ? `Familiar ${session.version}` : 'Familiar'}</p>
        <Tip label="Settings" shortcut={isMacKeyboard ? '⌘,' : 'Ctrl+,'}>
          <button type="button" className={styles.footerButton} onClick={() => openSettings()} aria-label="Settings">
            <Icon name="gear" size={17} weight={1.8} />
          </button>
        </Tip>
      </div>
      <NewMacSheet open={creating} onOpenChange={setCreating} />
    </nav>
  );
}

const COLLAPSED_KEY = 'dot-home:sidebar-collapsed';

/** Which sidebar groups are folded away, remembered in this browser. */
function useCollapsed(): [Set<string>, (id: string) => void] {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]') as string[]);
    } catch {
      return new Set();
    }
  });
  const toggle = (id: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  useEffect(() => {
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsed]));
    } catch {
      // Storage blocked: the choice lasts until reload.
    }
  }, [collapsed]);
  return [collapsed, toggle];
}

/** A source-list heading that folds its group, with the chevron shown on hover as on a Mac. */
function GroupTitle({ id, collapsed, onToggle, children }: { id: string; collapsed: Set<string>; onToggle: (id: string) => void; children: string }) {
  const open = !collapsed.has(id);
  return (
    <h2 className={styles.groupTitle}>
      <button type="button" className={styles.groupToggle} aria-expanded={open} aria-controls={`sidebar-${id}`} onClick={() => onToggle(id)}>
        {children}
        <span className={`${styles.groupChevron} ${open ? '' : styles.groupChevronClosed}`} aria-hidden="true">
          <Icon name="chevron-down" size={10} weight={2.6} />
        </span>
      </button>
    </h2>
  );
}

function PlaceItem({ href, icon, label, selected }: { href: string; icon: IconName; label: string; selected: boolean }) {
  return (
    <li>
      <a href={href} onClick={(event) => onLinkClick(event)} className={`${styles.item} ${selected ? styles.selected : ''}`} aria-current={selected ? 'page' : undefined}>
        <span className={styles.itemIcon}>
          <Icon name={icon} size={16} weight={1.8} />
        </span>
        <span className={styles.itemLabel}>{label}</span>
      </a>
    </li>
  );
}

function MacItem({ instance, expanded, section }: { instance: Instance; expanded: boolean; section: string | null }) {
  const info = instanceInfo(instance);
  const selected = expanded && section === 'overview';
  return (
    <li>
      <a
        href={macPath(instance.id)}
        onClick={(event) => onLinkClick(event)}
        className={`${styles.item} ${selected ? styles.selected : ''}`}
        aria-current={selected ? 'page' : undefined}
        aria-expanded={expanded}
      >
        <span className={`${styles.macGlyph} ${styles[`tone-${info.tone}`]}`}>
          <Icon name="display" size={16} weight={1.8} />
          <span className={styles.stateDot} />
        </span>
        <span className={styles.itemLabel}>
          {instance.name}
          <span className="visually-hidden">, {info.label}</span>
        </span>
        {instance.paused ? (
          <span className={styles.paused} title="Assistant paused">
            <Icon name="pause" size={12} weight={2.6} label="Assistant paused" />
          </span>
        ) : null}
        <CountBadge tone="sidebar" count={instance.counts.pending_approvals} label={`${instance.counts.pending_approvals} waiting for approval`} />
      </a>
      <Fold open={expanded}>
        <ul className={styles.sub}>
          {MAC_NAV.slice(1).map((item) => {
            const active = section === item.section;
            return (
              <li key={item.section}>
                <a href={macPath(instance.id, item.section)} onClick={(event) => onLinkClick(event)} className={`${styles.item} ${styles.subItem} ${active ? styles.selected : ''}`} aria-current={active ? 'page' : undefined}>
                  <span className={styles.itemIcon}>
                    <Icon name={item.icon} size={15} weight={1.8} />
                  </span>
                  <span className={styles.itemLabel}>{item.label}</span>
                  {item.section === 'review' ? <CountBadge tone="sidebar" {...sectionBadge('review', instance.counts)!} /> : null}
                </a>
              </li>
            );
          })}
        </ul>
      </Fold>
    </li>
  );
}

/**
 * Opens and closes a run of sidebar rows by animating their height, the way a
 * source list discloses a group. Closed rows are inert, so Tab skips them.
 */
function Fold({ open, id, children }: { open: boolean; id?: string; children: ReactNode }) {
  return (
    <div id={id} className={styles.fold} data-open={open || undefined} inert={!open}>
      <div className={styles.foldInner}>{children}</div>
    </div>
  );
}
