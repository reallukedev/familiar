import styles from './Spinner.module.css';

const SPOKES = Array.from({ length: 8 }, (_, i) => i);

/** The iOS activity indicator: eight fading spokes. */
export function Spinner({ size = 20, label }: { size?: number; label?: string }) {
  return (
    <span className={styles.spinner} style={{ width: size, height: size }} role={label ? 'status' : undefined} aria-label={label} aria-hidden={label ? undefined : true}>
      {SPOKES.map((i) => (
        <i key={i} style={{ transform: `rotate(${i * 45}deg)`, animationDelay: `${(i - 8) * 0.1}s` }} />
      ))}
    </span>
  );
}
