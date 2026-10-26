import { Dialog } from '@base-ui/react/dialog';
import { Drawer } from '@base-ui/react/drawer';
import type { ReactNode } from 'react';
import { usePlatform } from '../lib/platform';
import { Button } from './Button';
import { Spinner } from './Spinner';
import styles from './Sheet.module.css';

export type SheetPrimary = {
  label: string;
  /** Submit this form id; otherwise call onClick. */
  form?: string;
  onClick?: () => void;
  disabled?: boolean;
  busy?: boolean;
  tone?: 'default' | 'destructive';
};

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  primary?: SheetPrimary;
  /** "Cancel" when there is a primary action, "Done" when the sheet only shows something. */
  dismissLabel?: string;
  children: ReactNode;
  size?: 'regular' | 'large';
  /** False hides the dismiss button, for a sheet that must be finished (the recovery key). */
  dismissible?: boolean;
};

/**
 * A modal task. On a phone it is an iOS sheet that rises from the bottom with a
 * grabber and follows a swipe down; on a Mac it is a centered sheet with the
 * buttons at the bottom right. Both trap focus, close on Escape and give focus
 * back to whatever opened them.
 */
export function Sheet(props: Props) {
  return usePlatform() === 'phone' ? <PhoneSheet {...props} /> : <MacSheet {...props} />;
}

function PhoneSheet({ open, onOpenChange, title, description, primary, dismissLabel, children, dismissible = true }: Props) {
  const dismiss = dismissLabel ?? (primary ? 'Cancel' : 'Done');
  return (
    <Drawer.Root open={open} onOpenChange={(next) => onOpenChange(next)} swipeDirection="down">
      <Drawer.VirtualKeyboardProvider>
        <Drawer.Portal>
          <Drawer.Backdrop className={styles.backdrop} />
          <Drawer.Viewport className={styles.phoneViewport}>
            <Drawer.Popup className={styles.phonePopup}>
              <div className={styles.phoneBar}>
                <span className={styles.grabber} aria-hidden="true" />
                {dismissible ? <Drawer.Close className={styles.barButton}>{dismiss}</Drawer.Close> : <span />}
                <Drawer.Title className={`t-headline ${styles.barTitle}`}>{title}</Drawer.Title>
                {primary ? (
                  <button
                    type={primary.form ? 'submit' : 'button'}
                    form={primary.form}
                    onClick={primary.onClick}
                    disabled={primary.disabled || primary.busy}
                    aria-busy={primary.busy || undefined}
                    className={`${styles.barButton} ${styles.barPrimary} ${primary.tone === 'destructive' ? styles.barDestructive : ''}`}
                  >
                    {primary.busy ? <Spinner size={18} label={`${primary.label}…`} /> : primary.label}
                  </button>
                ) : (
                  <span />
                )}
              </div>
              <Drawer.Content className={styles.phoneBody}>
                {description ? <Drawer.Description className={`t-subhead secondary ${styles.description}`}>{description}</Drawer.Description> : null}
                {children}
              </Drawer.Content>
            </Drawer.Popup>
          </Drawer.Viewport>
        </Drawer.Portal>
      </Drawer.VirtualKeyboardProvider>
    </Drawer.Root>
  );
}

function MacSheet({ open, onOpenChange, title, description, primary, dismissLabel, children, size = 'regular', dismissible = true }: Props) {
  const dismiss = dismissLabel ?? (primary ? 'Cancel' : 'Done');
  return (
    <Dialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <Dialog.Portal>
        <Dialog.Backdrop className={`${styles.backdrop} ${styles.macBackdrop}`} />
        <Dialog.Viewport className={styles.macViewport}>
          <Dialog.Popup className={`${styles.macPopup} ${size === 'large' ? styles.large : ''}`}>
            <div className={styles.macBody}>
              <Dialog.Title className={`t-title2 ${styles.macTitle}`}>{title}</Dialog.Title>
              {description ? <Dialog.Description className={`t-body secondary ${styles.description}`}>{description}</Dialog.Description> : null}
              {children}
            </div>
            <div className={styles.macActions}>
              {dismissible ? <Dialog.Close render={<Button variant="gray">{dismiss}</Button>} /> : null}
              {primary ? (
                <Button
                  variant={primary.tone === 'destructive' ? 'destructive-filled' : 'filled'}
                  type={primary.form ? 'submit' : 'button'}
                  form={primary.form}
                  onClick={primary.onClick}
                  disabled={primary.disabled}
                  busy={primary.busy}
                >
                  {primary.label}
                </Button>
              ) : null}
            </div>
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
