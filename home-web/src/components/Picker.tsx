import { Select } from '@base-ui/react/select';
import { Icon } from './Icon';
import styles from './Picker.module.css';

type Option<T extends string> = { value: T; label: string };

/** A pop-up button: the macOS menu picker and the iOS menu-style picker in a row. */
export function Picker<T extends string>({ value, options, onChange, label, disabled }: { value: T; options: readonly Option<T>[]; onChange: (value: T) => void; label: string; disabled?: boolean }) {
  return (
    <Select.Root items={options as Option<T>[]} value={value} onValueChange={(next) => next !== null && onChange(next as T)} disabled={disabled}>
      <Select.Trigger className={styles.trigger} aria-label={label}>
        <Select.Value className={styles.value} />
        <Select.Icon className={styles.icon}>
          <Icon name="chevron-up-down" size={14} weight={2.2} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner className={styles.positioner} sideOffset={4} alignItemWithTrigger={false} align="end">
          <Select.Popup className={styles.popup}>
            <Select.List className={styles.list}>
              {options.map((option) => (
                <Select.Item key={option.value} value={option.value} className={styles.item}>
                  <Select.ItemIndicator className={styles.check}>
                    <Icon name="check" size={13} weight={2.6} />
                  </Select.ItemIndicator>
                  <Select.ItemText>{option.label}</Select.ItemText>
                </Select.Item>
              ))}
            </Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  );
}
