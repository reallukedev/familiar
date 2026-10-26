import { Toggle } from '@base-ui/react/toggle';
import { ToggleGroup } from '@base-ui/react/toggle-group';
import type { CSSProperties } from 'react';
import styles from './Segmented.module.css';

export type SegmentOption<T extends string> = { value: T; label: string; hint?: string };

type Props<T extends string> = {
  value: T;
  options: readonly SegmentOption<T>[];
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
  size?: 'regular' | 'compact';
};

/** The iOS / macOS segmented control. One option is always selected. */
export function Segmented<T extends string>({ value, options, onChange, label, disabled, size = 'regular' }: Props<T>) {
  const index = Math.max(0, options.findIndex((option) => option.value === value));
  const style = { '--count': options.length, '--index': index } as CSSProperties;
  return (
    <ToggleGroup
      value={[value]}
      onValueChange={(next) => {
        const picked = next[0] as T | undefined;
        if (picked && picked !== value) onChange(picked);
      }}
      aria-label={label}
      disabled={disabled}
      className={`${styles.track} ${styles[size]}`}
      style={style}
    >
      <span className={styles.thumb} aria-hidden="true" />
      {options.map((option) => (
        <Toggle key={option.value} value={option.value} className={styles.segment} title={option.hint} aria-label={option.hint ? `${option.label}: ${option.hint}` : undefined}>
          {/* The bold copy holds the width, so selecting never reflows the label. */}
          <span className={styles.label} data-text={option.label}>
            {option.label}
          </span>
        </Toggle>
      ))}
    </ToggleGroup>
  );
}
