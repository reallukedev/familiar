import type { ReactNode } from 'react';
import { Familiar, type FamiliarState } from './Familiar';
import { initials } from '../lib/format';
import type { Tone } from '../lib/status';
import { Icon, type IconName } from './Icon';
import styles from './Bits.module.css';

export function Avatar({ name, size = 'regular' }: { name: string; size?: 'regular' | 'large' | 'small' }) {
  return (
    <span className={`${styles.avatar} ${styles[`avatar-${size}`]}`} aria-hidden="true">
      {initials(name)}
    </span>
  );
}

/** A status dot and label. Color is never the only signal: the label always says it. */
export function StatusLabel({ tone, children, pulse }: { tone: Tone; children: ReactNode; pulse?: boolean }) {
  return (
    <span className={`${styles.status} ${styles[`tone-${tone}`]}`}>
      <span className={`${styles.dot} ${pulse ? styles.pulse : ''}`} aria-hidden="true" />
      <span>{children}</span>
    </span>
  );
}

export function Pill({ tone, children }: { tone: Tone; children: ReactNode }) {
  return <span className={`${styles.pill} ${styles[`pill-${tone}`]}`}>{children}</span>;
}

/** A count badge, as on a sidebar or tab item. */
/**
 * A count. orange: something waits for you (default). red: the iOS tab bar badge.
 * sidebar: a macOS source-list count, quiet gray, white on a selected row.
 */
export function CountBadge({ count, label, tone = 'orange' }: { count: number; label: string; tone?: 'orange' | 'red' | 'sidebar' }) {
  if (count <= 0) return null;
  return (
    <span className={`${styles.badge} ${styles[`badge-${tone}`]}`} aria-label={label}>
      {count > 99 ? '99+' : count}
    </span>
  );
}

/** A full-width notice inside a page: offline, paused, missing permissions. */
export function Banner({ tone, icon, title, children, action }: { tone: 'orange' | 'red' | 'blue' | 'gray'; icon: IconName; title: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className={`${styles.banner} ${styles[`banner-${tone}`]}`} role={tone === 'red' ? 'alert' : 'status'}>
      <span className={styles.bannerIcon}>
        <Icon name={icon} size={20} weight={2} />
      </span>
      <div className={styles.bannerText}>
        <p className="t-headline">{title}</p>
        {children ? <div className="t-subhead">{children}</div> : null}
      </div>
      {action ? <div className={styles.bannerAction}>{action}</div> : null}
    </div>
  );
}

/**
 * What a page shows before it has anything, with the one thing to do next.
 * `familiar` puts the mascot in the icon's place, for the few states it speaks to.
 */
export function EmptyState({ icon, familiar, title, children, action }: { icon?: IconName; familiar?: FamiliarState; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className={styles.empty}>
      <span className={`${styles.emptyIcon} ${familiar ? styles.emptyFamiliar : ''}`} aria-hidden="true">
        {familiar ? <Familiar state={familiar} size={64} /> : icon ? <Icon name={icon} size={52} weight={1.4} /> : null}
      </span>
      <h2 className={`balance ${styles.emptyTitle}`}>{title}</h2>
      {children ? <div className={`pretty ${styles.emptyText}`}>{children}</div> : null}
      {action ? <div className={styles.emptyAction}>{action}</div> : null}
    </div>
  );
}
