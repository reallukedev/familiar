import { Field } from '@base-ui/react/field';
import { OTPField } from '@base-ui/react/otp-field';
import { useId, type ComponentProps, type ReactNode } from 'react';
import styles from './Field.module.css';

type TextFieldProps = Omit<ComponentProps<'input'>, 'className' | 'children' | 'onChange' | 'value'> & {
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  description?: ReactNode;
  /** A message shown under the field; the field is marked invalid while set. */
  error?: string | null;
  /** Visually hide the label (it stays the field's accessible name). */
  labelHidden?: boolean;
  trailing?: ReactNode;
  mono?: boolean;
};

export function TextField({ label, value, onValueChange, description, error, labelHidden, trailing, mono, ...input }: TextFieldProps) {
  return (
    <Field.Root className={styles.field} invalid={!!error}>
      <Field.Label className={labelHidden ? 'visually-hidden' : `t-footnote ${styles.label}`}>{label}</Field.Label>
      <div className={styles.control}>
        <Field.Control
          className={`${styles.input} ${mono ? styles.mono : ''}`}
          value={value}
          onValueChange={(next) => onValueChange(String(next))}
          {...input}
        />
        {trailing}
      </div>
      {error ? (
        <Field.Error className={`t-footnote ${styles.error}`} match>
          {error}
        </Field.Error>
      ) : description ? (
        <Field.Description className={`t-footnote secondary ${styles.description}`}>{description}</Field.Description>
      ) : null}
    </Field.Root>
  );
}

type AreaProps = Omit<ComponentProps<'textarea'>, 'className' | 'onChange' | 'value'> & {
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  description?: ReactNode;
  error?: string | null;
};

export function TextArea({ label, value, onValueChange, description, error, id: given, ...rest }: AreaProps) {
  const generated = useId();
  const id = given ?? generated;
  return (
    <Field.Root className={styles.field} invalid={!!error}>
      <Field.Label className={`t-footnote ${styles.label}`} htmlFor={id}>
        {label}
      </Field.Label>
      <Field.Control
        render={<textarea id={id} {...rest} />}
        className={`${styles.input} ${styles.area}`}
        value={value}
        onValueChange={(next) => onValueChange(String(next))}
      />
      {error ? (
        <Field.Error className={`t-footnote ${styles.error}`} match>
          {error}
        </Field.Error>
      ) : description ? (
        <Field.Description className={`t-footnote secondary ${styles.description}`}>{description}</Field.Description>
      ) : null}
    </Field.Root>
  );
}

/** Six digits from an authenticator app. One real field for paste and autofill. */
export function CodeField({ value, onValueChange, onComplete, invalid, disabled, label = 'Six-digit code' }: { value: string; onValueChange: (value: string) => void; onComplete?: (code: string) => void; invalid?: boolean; disabled?: boolean; label?: string }) {
  const id = useId();
  return (
    <div className={styles.code}>
      <label htmlFor={id} className="visually-hidden">
        {label}
      </label>
      <OTPField.Root
        id={id}
        length={6}
        value={value}
        onValueChange={(next) => onValueChange(next)}
        onValueComplete={(next) => onComplete?.(next)}
        validationType="numeric"
        inputMode="numeric"
        autoComplete="one-time-code"
        disabled={disabled}
        aria-invalid={invalid || undefined}
        className={`${styles.codeRoot} ${invalid ? styles.codeInvalid : ''}`}
      >
        {Array.from({ length: 6 }, (_, index) => (
          <OTPField.Input key={index} className={styles.codeCell} aria-label={index === 0 ? undefined : `Digit ${index + 1} of 6`} />
        ))}
      </OTPField.Root>
    </div>
  );
}

/** A form-level error, for messages that don't belong to one field. */
export function FormError({ children }: { children: ReactNode }) {
  if (!children) return null;
  return (
    <p className={`t-footnote ${styles.formError}`} role="alert">
      {children}
    </p>
  );
}

export function Fields({ children }: { children: ReactNode }) {
  return <div className={styles.fields}>{children}</div>;
}
