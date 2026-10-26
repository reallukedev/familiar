import type { CSSProperties, ReactNode } from 'react';
import { bytes } from '../../lib/format';
import type { Segment } from '../../lib/storage';
import styles from './CapacityBar.module.css';

type Mark = { at: number; label: string };

type Props = {
  segments: Segment[];
  /** What the full width stands for, in bytes. */
  scale: number;
  /** Caption for the hidden table that reads the bar out. */
  caption: string;
  /** A labelled tick on the track: the quota when usage passed it, or the server's reserve. */
  mark?: Mark;
  size?: 'large' | 'thin';
};

const pct = (value: number, scale: number) => (scale > 0 ? Math.min(100, Math.max(0, (value / scale) * 100)) : 0);

/**
 * The storage bar from macOS Settings: one track, one colored run per kind,
 * separated by a hairline of the surface. Screen readers get the same numbers
 * from a visually hidden table instead of the picture.
 */
export function CapacityBar({ segments, scale, caption, mark, size = 'large' }: Props) {
  const shown = segments.filter((segment) => segment.tone !== 'free');
  return (
    <div className={`${styles.root} ${styles[size]}`}>
      <div className={styles.track} aria-hidden="true">
        <div className={styles.fill}>
          {shown.map((segment) => (
            <span key={segment.key} className={`${styles.segment} ${styles[`tone-${segment.tone}`]}`} style={{ '--w': `${pct(segment.bytes, scale)}%` } as CSSProperties} title={`${segment.label}: ${bytes(segment.bytes)}`} />
          ))}
        </div>
        {mark ? <span className={styles.mark} style={{ '--at': `${pct(mark.at, scale)}%` } as CSSProperties} /> : null}
      </div>
      {size === 'large' ? (
        <table className="visually-hidden">
          <caption>{caption}</caption>
          <tbody>
            {segments.map((segment) => (
              <tr key={segment.key}>
                <th scope="row">{segment.label}</th>
                <td>{bytes(segment.bytes)}</td>
              </tr>
            ))}
            {mark ? (
              <tr>
                <th scope="row">{mark.label}</th>
                <td>{bytes(mark.at)}</td>
              </tr>
            ) : null}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}

/** The colored key for one segment. */
export function Swatch({ tone }: { tone: Segment['tone'] }) {
  return <span className={`${styles.swatch} ${styles[`tone-${tone}`]}`} aria-hidden="true" />;
}

/** Legend under the bar: dot and name, as macOS shows it; sizes are on hover. Visual only; the bar's table is what's read out. */
export function Legend({ segments, extra }: { segments: Segment[]; extra?: ReactNode }) {
  return (
    <ul className={styles.legend} aria-hidden="true">
      {segments.map((segment) => (
        <li key={segment.key} className={styles.legendItem}>
          <Swatch tone={segment.tone} />
          <span className={styles.legendLabel}>{segment.label}</span>
        </li>
      ))}
      {extra}
    </ul>
  );
}
