import { Meter as BaseMeter } from '@base-ui/react/meter';
import { Progress } from '@base-ui/react/progress';
import styles from './Meter.module.css';

/** A storage-style bar: label and value on one line, the bar under it. */
export function Meter({ label, value, max, valueText, warnAt = 0.85 }: { label: string; value: number; max: number; valueText: string; warnAt?: number }) {
  const ratio = max > 0 ? value / max : 0;
  const tone = ratio >= 0.95 ? 'red' : ratio >= warnAt ? 'orange' : 'blue';
  return (
    <BaseMeter.Root value={Math.min(value, max)} max={max} className={styles.meter} getAriaValueText={() => valueText}>
      <div className={styles.line}>
        <BaseMeter.Label className={`t-body ${styles.label}`}>{label}</BaseMeter.Label>
        <span className="t-subhead secondary tabular">{valueText}</span>
      </div>
      <BaseMeter.Track className={styles.track}>
        <BaseMeter.Indicator className={`${styles.indicator} ${styles[tone]}`} />
      </BaseMeter.Track>
    </BaseMeter.Root>
  );
}

/** Determinate when `value` is known, otherwise an honest indeterminate bar. */
export function ProgressBar({ value, label, tone = 'blue' }: { value: number | null; label: string; tone?: 'blue' | 'light' }) {
  return (
    <Progress.Root value={value === null ? null : Math.round(value * 100)} max={100} className={styles.progress} aria-label={label}>
      <Progress.Track className={`${styles.track} ${tone === 'light' ? styles.lightTrack : ''}`}>
        <Progress.Indicator className={`${styles.indicator} ${tone === 'light' ? styles.light : styles.blue} ${value === null ? styles.indeterminate : ''}`} />
      </Progress.Track>
    </Progress.Root>
  );
}
