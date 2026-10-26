import type { ReactNode } from 'react';
import { Familiar, type FamiliarState } from '../components/Familiar';
import styles from './AuthLayout.module.css';

/** A standalone screen (sign-in, setup, invite, consent): one centered card. */
export function AuthLayout({ title, children, footer, wide, mood = 'idle' }: { title: string; children: ReactNode; footer?: ReactNode; wide?: boolean; mood?: FamiliarState }) {
  return (
    <main className={styles.screen}>
      <div className={`${styles.card} ${wide ? styles.wide : ''}`}>
        <Familiar className={styles.mark} state={mood} size={56} />
        <h1 className={`t-title1 balance ${styles.title}`}>{title}</h1>
        {children}
      </div>
      {footer ? <div className={`t-footnote secondary ${styles.footer}`}>{footer}</div> : null}
    </main>
  );
}
