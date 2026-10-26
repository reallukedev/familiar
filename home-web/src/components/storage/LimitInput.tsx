import type { ComponentProps } from 'react';
import styles from './LimitInput.module.css';

type Props = Omit<ComponentProps<'input'>, 'className' | 'onChange' | 'value' | 'type'> & {
  value: string;
  onValueChange: (value: string) => void;
  /** Shown after the number inside the field: "GB". */
  unit?: string;
  invalid?: boolean;
};

/**
 * A short number field that sits at the trailing edge of a row, as in System
 * Settings: right-aligned digits with the unit after them. The row's title is
 * its label (pass aria-label or aria-labelledby).
 */
export function LimitInput({ value, onValueChange, unit, invalid, disabled, ...rest }: Props) {
  return (
    <span className={styles.control} data-invalid={invalid || undefined} data-disabled={disabled || undefined}>
      <input
        type="text"
        inputMode="numeric"
        autoComplete="off"
        spellCheck={false}
        enterKeyHint="done"
        className={`tabular ${styles.input}`}
        value={value}
        onChange={(event) => onValueChange(event.target.value)}
        aria-invalid={invalid || undefined}
        disabled={disabled}
        {...rest}
      />
      {unit ? (
        <span className={styles.unit} aria-hidden="true">
          {unit}
        </span>
      ) : null}
    </span>
  );
}
