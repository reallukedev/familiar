import styles from './Skeleton.module.css';

/** Placeholder bars shaped like the content that will replace them. */
export function SkeletonText({ width = '60%', size = 'body' }: { width?: string; size?: 'body' | 'subhead' | 'title' }) {
  return <span className={`${styles.bar} ${styles[size]}`} style={{ width }} aria-hidden="true" />;
}

export function SkeletonRows({ rows = 3, leading = false, label = 'Loading' }: { rows?: number; leading?: boolean; label?: string }) {
  return (
    <div className={styles.rows} role="status" aria-label={label}>
      {Array.from({ length: rows }, (_, index) => (
        <div className={styles.row} key={index}>
          {leading ? <span className={styles.tile} /> : null}
          <span className={styles.text}>
            <span className={`${styles.bar} ${styles.body}`} style={{ width: `${46 + ((index * 17) % 30)}%` }} />
            <span className={`${styles.bar} ${styles.subhead}`} style={{ width: `${28 + ((index * 23) % 26)}%` }} />
          </span>
        </div>
      ))}
    </div>
  );
}

export function SkeletonBlock({ height, radius = 12 }: { height: number | string; radius?: number }) {
  return <div className={styles.block} style={{ height, borderRadius: radius }} aria-hidden="true" />;
}
