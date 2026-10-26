import { useId, useState } from 'react';
import { usePlatform } from '../lib/platform';
import { Icon } from './Icon';
import { Picker } from './Picker';
import { Sheet } from './Sheet';
import styles from './Choice.module.css';

export type ChoiceOption<T extends string> = { value: T; label: string; detail?: string };

type Props<T extends string> = {
  /** Spoken name of the control, and the sheet's title on a phone. */
  label: string;
  value: T;
  options: readonly ChoiceOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
};

/**
 * One of a few choices. On a Mac it is the pop-up button System Settings uses;
 * on a phone it opens a sheet with a checked list, where each choice can carry
 * a line saying what it does.
 */
export function Choice<T extends string>(props: Props<T>) {
  return usePlatform() === 'phone' ? <PhoneChoice {...props} /> : <Picker label={props.label} value={props.value} options={props.options} onChange={props.onChange} disabled={props.disabled} />;
}

function PhoneChoice<T extends string>({ label, value, options, onChange, disabled }: Props<T>) {
  const [open, setOpen] = useState(false);
  const current = options.find((option) => option.value === value);
  const group = useId();
  return (
    <>
      <button type="button" className={styles.trigger} onClick={() => setOpen(true)} disabled={disabled} aria-haspopup="dialog" aria-label={`${label}: ${current?.label ?? ''}`}>
        <span className={styles.value}>{current?.label}</span>
        <Icon name="chevron-up-down" size={14} weight={2.2} className={styles.icon} />
      </button>
      <Sheet open={open} onOpenChange={setOpen} title={label} dismissLabel="Done">
        <div className={styles.list} role="radiogroup" aria-labelledby={group}>
          <span id={group} className="visually-hidden">
            {label}
          </span>
          {options.map((option) => {
            const checked = option.value === value;
            return (
              <button
                key={option.value}
                type="button"
                role="radio"
                aria-checked={checked}
                className={styles.option}
                onClick={() => {
                  setOpen(false);
                  if (!checked) onChange(option.value);
                }}
              >
                <span className={styles.text}>
                  <span className={styles.label}>{option.label}</span>
                  {option.detail ? <span className={`${styles.detail} secondary`}>{option.detail}</span> : null}
                </span>
                <span className={styles.check} aria-hidden="true">
                  {checked ? <Icon name="check" size={17} weight={2.6} /> : null}
                </span>
              </button>
            );
          })}
        </div>
      </Sheet>
    </>
  );
}
