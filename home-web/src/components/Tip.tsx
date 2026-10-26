import { Tooltip } from '@base-ui/react/tooltip';
import type { ReactElement } from 'react';
import styles from './Tip.module.css';

/** A hover hint for icon buttons on a Mac. Base UI keeps tooltips off touch screens. */
export function Tip({ label, children, shortcut }: { label: string; children: ReactElement; shortcut?: string }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger render={children} />
      <Tooltip.Portal>
        <Tooltip.Positioner sideOffset={8} className={styles.positioner}>
          <Tooltip.Popup className={styles.popup}>
            {label}
            {shortcut ? <kbd className={styles.kbd}>{shortcut}</kbd> : null}
          </Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
