import { useState } from 'react';
import { flushSync } from 'react-dom';
import { Button } from '../components/Button';
import { FAMILIAR_STATES, Familiar, type FamiliarState } from '../components/Familiar';
import { Segmented } from '../components/Segmented';
import styles from './FamiliarGallery.module.css';

// Development only: every Familiar state side by side, for review. Open with ?demo&familiar.
const SIZES = [20, 28, 40, 56, 96, 160];

declare global {
  interface Window {
    __familiar?: (state: FamiliarState) => void;
  }
}

export function FamiliarGallery() {
  const params = new URLSearchParams(location.search);
  if (params.has('stage')) return <Stage state={(params.get('stage') || 'idle') as FamiliarState} dark={params.get('bg') === 'dark'} size={Number(params.get('size')) || 240} />;
  return <Gallery />;
}

/** One Familiar alone, for recording: ?demo&familiar&stage=idle&bg=dark&size=240. Drive it with window.__familiar(state). */
function Stage({ state: initial, dark, size }: { state: FamiliarState; dark: boolean; size: number }) {
  const [state, setState] = useState(initial);
  window.__familiar = (next) => flushSync(() => setState(next));
  return (
    <main className={styles.stageOnly} data-surface={dark ? 'dark' : 'light'}>
      <Familiar state={state} size={size} variant={dark ? 'glyph' : 'icon'} />
    </main>
  );
}

function Gallery() {
  const [live, setLive] = useState<FamiliarState>('idle');
  const [surface, setSurface] = useState<'light' | 'dark'>('light');
  return (
    <main className={styles.page} data-surface={surface}>
      <header className={styles.header}>
        <h1 className="t-title1">Familiar</h1>
        <Segmented label="Surface" value={surface} onChange={setSurface} options={[{ value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }]} size="compact" />
      </header>

      <section className={styles.stage} aria-label="Live">
        <div className={styles.live} data-testid="live">
          <Familiar state={live} size={160} />
          <span className={styles.screen}>
            <Familiar state={live} size={160} variant="glyph" />
          </span>
        </div>
        <div className={styles.buttons}>
          {FAMILIAR_STATES.map((state) => (
            <Button key={state} variant={state === live ? 'filled' : 'gray'} onClick={() => setLive(state)} data-state={state}>
              {state}
            </Button>
          ))}
        </div>
        <div className={styles.ladder}>
          {SIZES.map((size) => (
            <figure key={size} className={styles.rung}>
              <Familiar state={live} size={size} />
              <figcaption className="t-caption secondary tabular">{size}</figcaption>
            </figure>
          ))}
          {SIZES.map((size) => (
            <figure key={`g${size}`} className={`${styles.rung} ${styles.dark}`}>
              <Familiar state={live} size={size} variant="glyph" />
              <figcaption className="t-caption tabular">{size}</figcaption>
            </figure>
          ))}
        </div>
      </section>

      <section className={styles.grid} aria-label="Every state">
        {FAMILIAR_STATES.map((state) => (
          <figure key={state} className={styles.cell} data-cell={state}>
            <div className={styles.pair}>
              <Familiar state={state} size={96} />
              <span className={styles.screenSmall}>
                <Familiar state={state} size={72} variant="glyph" />
              </span>
            </div>
            <figcaption className="t-headline">{state}</figcaption>
          </figure>
        ))}
      </section>
    </main>
  );
}
