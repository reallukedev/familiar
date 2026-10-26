import { Switch as BaseSwitch } from '@base-ui/react/switch';
import styles from './Switch.module.css';

type Props = {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  /** Orange is reserved for the pause switch (attention, not "on"). */
  tone?: 'green' | 'orange';
  id?: string;
};

export function Switch({ checked, onCheckedChange, label, disabled, tone = 'green', id }: Props) {
  return (
    <BaseSwitch.Root
      checked={checked}
      onCheckedChange={(value) => onCheckedChange(value)}
      disabled={disabled}
      aria-label={label}
      id={id}
      className={`${styles.switch} ${styles[tone]}`}
    >
      <BaseSwitch.Thumb className={styles.thumb} />
    </BaseSwitch.Root>
  );
}
