import { Menu } from '@base-ui/react/menu';
import type { ReactElement, ReactNode } from 'react';
import { usePlatform } from '../lib/platform';
import styles from './Menu.module.css';

export type MenuAction = {
  label: string;
  onSelect: () => void;
  tone?: 'default' | 'destructive';
  disabled?: boolean;
  hint?: string;
  /** Start a new section above this item. */
  separated?: boolean;
};

/**
 * A pull-down menu. iOS context-menu proportions on a phone, a compact macOS
 * menu with the blue highlight on a Mac. Opens instantly: it's used often.
 */
export function ActionMenu({ trigger, items, label, align = 'end' }: { trigger: ReactElement; items: MenuAction[]; label: string; align?: 'start' | 'end' | 'center' }) {
  const phone = usePlatform() === 'phone';
  return (
    <Menu.Root>
      <Menu.Trigger render={trigger} aria-label={label} />
      <Menu.Portal>
        <Menu.Positioner className={styles.positioner} sideOffset={6} align={align} collisionPadding={12}>
          <Menu.Popup className={`${styles.popup} ${phone ? styles.phone : styles.mac}`}>
            {items.map((item) => (
              <MenuRow key={item.label} item={item} />
            ))}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

function MenuRow({ item }: { item: MenuAction }) {
  return (
    <>
      {item.separated ? <Menu.Separator className={styles.separator} /> : null}
      <Menu.Item className={`${styles.item} ${item.tone === 'destructive' ? styles.destructive : ''}`} onClick={item.onSelect} disabled={item.disabled}>
        <span className={styles.itemLabel}>{item.label}</span>
        {item.hint ? <kbd className={styles.hint}>{item.hint}</kbd> : null}
      </Menu.Item>
    </>
  );
}

export function MenuTriggerLabel({ children }: { children: ReactNode }) {
  return <span className={styles.triggerLabel}>{children}</span>;
}
