import { AlertDialog } from '@base-ui/react/alert-dialog';
import { useId, useRef, useState, type ReactNode } from 'react';
import { usePlatform } from '../lib/platform';
import styles from './Confirm.module.css';

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  message: ReactNode;
  action: string;
  onConfirm: () => void;
  tone?: 'destructive' | 'default';
  /**
   * For warnings people may want to stop seeing: shows "Don't ask again",
   * remembered in this browser under this key. Check skipsConfirm(key) first.
   */
  remember?: string;
};

const SKIP = 'dot-home:skip-confirm:';

/** True when someone chose "Don't ask again" for this warning in this browser. */
export function skipsConfirm(key: string): boolean {
  try {
    return localStorage.getItem(SKIP + key) === '1';
  } catch {
    return false;
  }
}

/**
 * A confirmation before something that can't be undone. On a phone it is the
 * iOS action sheet; on a Mac, a compact alert with the buttons stacked.
 * Focus starts on Cancel so Return never destroys anything by accident.
 */
export function Confirm({ open, onOpenChange, title, message, action, onConfirm, tone = 'destructive', remember }: Props) {
  const phone = usePlatform() === 'phone';
  const cancel = useRef<HTMLButtonElement>(null);
  const [skip, setSkip] = useState(false);
  const skipId = useId();
  const confirm = () => {
    if (remember && skip) {
      try {
        localStorage.setItem(SKIP + remember, '1');
      } catch {
        // Storage blocked: it asks again next time.
      }
    }
    onConfirm();
  };
  return (
    <AlertDialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <AlertDialog.Portal>
        <AlertDialog.Backdrop className={styles.backdrop} />
        <AlertDialog.Viewport className={phone ? styles.phoneViewport : styles.macViewport}>
          <AlertDialog.Popup className={phone ? styles.phonePopup : styles.macPopup} initialFocus={cancel}>
            <div className={phone ? styles.card : styles.macCard}>
              <div className={styles.message}>
                <AlertDialog.Title className={phone ? `t-footnote ${styles.phoneTitle}` : `t-headline ${styles.macTitle}`}>{title}</AlertDialog.Title>
                <AlertDialog.Description className={`${phone ? 't-footnote' : 't-subhead'} secondary`}>{message}</AlertDialog.Description>
                {remember ? (
                  <label className={styles.skip} htmlFor={skipId}>
                    <input id={skipId} type="checkbox" checked={skip} onChange={(event) => setSkip(event.target.checked)} />
                    Don’t ask again
                  </label>
                ) : null}
              </div>
              <div className={phone ? undefined : styles.macButtons}>
                <AlertDialog.Close
                  className={`${phone ? styles.choice : styles.macButton} ${tone === 'destructive' ? styles.destructive : styles.primary}`}
                  onClick={confirm}
                >
                  {action}
                </AlertDialog.Close>
                {phone ? null : (
                  <AlertDialog.Close className={`${styles.macButton} ${styles.cancel}`} ref={cancel}>
                    Cancel
                  </AlertDialog.Close>
                )}
              </div>
            </div>
            {phone ? (
              <AlertDialog.Close className={`${styles.choice} ${styles.phoneCancel}`} ref={cancel}>
                Cancel
              </AlertDialog.Close>
            ) : null}
          </AlertDialog.Popup>
        </AlertDialog.Viewport>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
