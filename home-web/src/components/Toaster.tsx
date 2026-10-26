import { Toast } from '@base-ui/react/toast';
import type { CSSProperties } from 'react';
import { usePlatform } from '../lib/platform';
import { Icon } from './Icon';
import styles from './Toaster.module.css';

/** Toasts: a pill at the top on a phone, a notification at the top right on a Mac. */
export function Toaster() {
  const { toasts } = Toast.useToastManager();
  const phone = usePlatform() === 'phone';
  return (
    <Toast.Portal>
      <Toast.Viewport className={`${styles.viewport} ${phone ? styles.phone : styles.mac}`}>
        {toasts.map((toast) => (
          <Toast.Root key={toast.id} toast={toast} className={`${styles.toast} ${styles[toast.type ?? 'info'] ?? ''}`} swipeDirection={phone ? 'up' : ['right', 'up']}>
            <Toast.Content className={styles.content}>
              {toast.type === 'success' ? (
                <span className={`${styles.glyph} ${styles.ok}`}>
                  <Icon name="check" size={16} weight={2.6} />
                </span>
              ) : toast.type === 'error' ? (
                <span className={`${styles.glyph} ${styles.bad}`}>
                  <Icon name="exclamation" size={17} weight={2.1} />
                </span>
              ) : null}
              <Toast.Title className={styles.title} />
              {toast.type === 'undo' ? (
                <>
                  {/* A ring that empties over the undo time, so the deadline is visible. */}
                  <span className={styles.countdown} style={{ '--seconds': `${(toast.data as { seconds?: number } | undefined)?.seconds ?? 3}s` } as CSSProperties} aria-hidden="true" />
                  <Toast.Action className={styles.action} />
                </>
              ) : (
                <Toast.Close className={styles.close} aria-label="Dismiss">
                  <Icon name="xmark" size={12} weight={2.4} />
                </Toast.Close>
              )}
            </Toast.Content>
          </Toast.Root>
        ))}
      </Toast.Viewport>
    </Toast.Portal>
  );
}
