import { Button as BaseButton } from '@base-ui/react/button';
import type { ComponentProps, ReactNode } from 'react';
import { Spinner } from './Spinner';
import styles from './Button.module.css';

export type ButtonVariant = 'filled' | 'tinted' | 'gray' | 'plain' | 'destructive' | 'destructive-filled' | 'on-dark';

type Props = Omit<ComponentProps<typeof BaseButton>, 'className' | 'children'> & {
  variant?: ButtonVariant;
  /** 'full' stretches to the container (iOS forms); 'auto' hugs the label (macOS). */
  width?: 'auto' | 'full';
  size?: 'regular' | 'large' | 'small';
  busy?: boolean;
  icon?: ReactNode;
  children: ReactNode;
};

export function Button({ variant = 'filled', width = 'auto', size = 'regular', busy = false, disabled, icon, children, type = 'button', ...rest }: Props) {
  return (
    <BaseButton
      type={type}
      className={[styles.button, styles[variant], size === 'regular' ? '' : styles[size], width === 'full' ? styles.full : ''].filter(Boolean).join(' ')}
      disabled={disabled || busy}
      focusableWhenDisabled={busy}
      aria-busy={busy || undefined}
      {...rest}
    >
      <span className={styles.label} data-hidden={busy || undefined}>
        {icon}
        {children}
      </span>
      {busy ? (
        <span className={styles.spin}>
          <Spinner size={18} />
        </span>
      ) : null}
    </BaseButton>
  );
}

/** A square glyph button for toolbars and rows. Always has a spoken name. */
export function IconButton({ label, children, variant = 'plain', ...rest }: Omit<ComponentProps<typeof BaseButton>, 'className' | 'children'> & { label: string; children: ReactNode; variant?: 'plain' | 'gray' | 'on-dark' }) {
  return (
    <BaseButton type="button" aria-label={label} className={`${styles.icon} ${styles[`icon-${variant}`]}`} {...rest}>
      {children}
    </BaseButton>
  );
}
