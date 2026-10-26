import { Dialog } from '@base-ui/react/dialog';
import { useEffect, useMemo, useState } from 'react';
import { Icon, type IconName } from '../components/Icon';
import { api } from '../lib/api';
import { fetchKey, keys, peek } from '../lib/cache';
import { navigate, useLocation } from '../lib/router';
import { adminPath, macPath } from '../lib/routes';
import { useSession } from '../lib/session';
import type { Instance, PermissionGroup } from '../lib/types';
import { ADMIN_NAV, MAC_NAV, recallMac } from './nav';
import styles from './CommandPalette.module.css';

type Item = { id: string; title: string; detail: string; icon: IconName; href: string; group: string };

/**
 * Jump anywhere, or find a tool the assistant can use. Opens with ⌘K or "/".
 * No open or close animation: it's a keyboard tool used many times a day.
 */
export function CommandPalette({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [tick, setTick] = useState(0);
  const { route } = useLocation();
  const { data: session } = useSession();
  const macId = route.name === 'mac' ? route.id : recallMac();

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    if (macId && !peek(keys.permissions(macId))) {
      void fetchKey(keys.permissions(macId), () => api.permissions(macId))
        .then(() => setTick((n) => n + 1))
        .catch(() => {});
    }
  }, [open, macId]);

  const items = useMemo(() => {
    const instances = peek<Instance[]>(keys.instances) ?? [];
    const list: Item[] = [];
    for (const instance of instances) list.push({ id: `mac-${instance.id}`, title: instance.name, detail: 'Mac', icon: 'display', href: macPath(instance.id), group: 'Macs' });
    const current = instances.find((instance) => instance.id === macId);
    if (current) {
      for (const item of MAC_NAV.slice(1)) list.push({ id: `sec-${item.section}`, title: item.label, detail: current.name, icon: item.icon, href: macPath(current.id, item.section), group: 'Sections' });
    }
    list.push({ id: 'account', title: 'Account', detail: 'Password, two-factor, sessions', icon: 'person', href: '/account', group: 'Sections' });
    if (session?.user?.role === 'admin') {
      for (const item of ADMIN_NAV) list.push({ id: `admin-${item.section}`, title: item.label, detail: 'Server', icon: item.icon, href: adminPath(item.section), group: 'Server' });
    }
    if (current) {
      const groups = peek<PermissionGroup[]>(keys.permissions(current.id)) ?? [];
      for (const group of groups) {
        for (const tool of group.tools) {
          list.push({ id: `tool-${tool.name}`, title: tool.title, detail: `${tool.name} · ${group.label}`, icon: 'terminal', href: `${macPath(current.id, 'permissions')}#group-${group.id}`, group: 'Tools' });
        }
      }
    }
    return list;
  }, [macId, session, open, tick]);

  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matched = q ? items.filter((item) => `${item.title} ${item.detail}`.toLowerCase().includes(q)) : items.filter((item) => item.group !== 'Tools');
    return matched.slice(0, 60);
  }, [items, query]);

  const choose = (item: Item | undefined) => {
    if (!item) return;
    onOpenChange(false);
    navigate(item.href);
  };

  return (
    <Dialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <Dialog.Portal>
        <Dialog.Backdrop className={styles.backdrop} />
        <Dialog.Viewport className={styles.viewport}>
          <Dialog.Popup className={styles.popup} aria-label="Search">
            <Dialog.Title className="visually-hidden">Search Familiar</Dialog.Title>
            <div className={styles.field}>
              <Icon name="search" size={18} weight={2} />
              <input
                className={styles.input}
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActive(0);
                }}
                placeholder="Search Macs, settings and tools…"
                aria-label="Search Macs, settings and tools"
                role="combobox"
                aria-expanded="true"
                aria-controls="palette-results"
                aria-activedescendant={results[active] ? `palette-${results[active].id}` : undefined}
                autoComplete="off"
                spellCheck={false}
                onKeyDown={(event) => {
                  if (event.key === 'ArrowDown') {
                    event.preventDefault();
                    setActive((i) => Math.min(results.length - 1, i + 1));
                  } else if (event.key === 'ArrowUp') {
                    event.preventDefault();
                    setActive((i) => Math.max(0, i - 1));
                  } else if (event.key === 'Enter') {
                    event.preventDefault();
                    choose(results[active]);
                  }
                }}
              />
            </div>
            <ul className={styles.results} id="palette-results" role="listbox" aria-label="Results">
              {results.map((item, index) => (
                <li
                  key={item.id}
                  id={`palette-${item.id}`}
                  role="option"
                  aria-selected={index === active}
                  className={`${styles.result} ${index === active ? styles.active : ''}`}
                  onMouseMove={() => setActive(index)}
                  onClick={() => choose(item)}
                >
                  <span className={styles.resultIcon}>
                    <Icon name={item.icon} size={16} weight={1.8} />
                  </span>
                  <span className={styles.resultTitle}>{item.title}</span>
                  <span className={styles.resultDetail}>{item.detail}</span>
                </li>
              ))}
              {results.length === 0 ? <li className={styles.none}>No matches for “{query}”.</li> : null}
            </ul>
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
