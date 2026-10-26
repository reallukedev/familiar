import type { ReactNode } from 'react';
import { hasScreen, instanceInfo, isLockedOff, progressOf, stateDetail } from '../lib/status';
import { unlockMacs } from '../lib/unlock';
import type { Instance, PowerAction } from '../lib/types';
import { Button } from './Button';
import { Familiar, type FamiliarState } from './Familiar';
import { Icon } from './Icon';
import { ProgressBar } from './Meter';
import { Spinner } from './Spinner';
import { useThumbnail } from './useThumbnail';
import styles from './MacScreen.module.css';

type Props = {
  instance: Instance;
  /** hero: the big display on a Mac's page; card: the list; tiny: a sidebar or row glyph. */
  size?: 'hero' | 'card' | 'tiny';
  /** Replaces the still frame with the live screen (noVNC canvas). */
  live?: ReactNode;
  onPower?: (action: PowerAction) => void;
  powerBusy?: boolean;
  /** Something to place over the picture's bottom edge (Show Screen). */
  action?: ReactNode;
};

/**
 * The Mac, presented as a display: a thin dark bezel around a 16:10 panel,
 * the latest frame inside, and the Mac's state laid over it when there's
 * nothing to see.
 */
export function MacScreen({ instance, size = 'hero', live, onPower, powerBusy, action }: Props) {
  const showsFrame = hasScreen(instance.state) && instance.state !== 'booting';
  const { ref, url } = useThumbnail(instance.id, showsFrame && !live, size === 'tiny' ? 15_000 : 5000);
  const info = instanceInfo(instance);
  const overlay = !live && !(showsFrame && url);
  const label = `${instance.name} screen. ${info.label}${instance.paused ? ', assistant paused' : ''}.`;

  return (
    <div className={`${styles.frame} ${styles[size]}`} ref={ref as React.RefObject<HTMLDivElement>}>
      <div className={styles.screen} role={live ? undefined : 'img'} aria-label={live ? undefined : label}>
        {live ?? (url ? <img className={styles.frameImage} src={url} alt="" width={640} height={400} draggable={false} /> : null)}
        {overlay ? <StateOverlay instance={instance} size={size} onPower={onPower} powerBusy={powerBusy} hasFrame={!!url} /> : null}
        {size !== 'tiny' && (instance.paused || instance.state === 'degraded') && !overlay ? (
          <div className={styles.chips}>
            {instance.paused ? (
              <span className={`${styles.chip} ${styles.chipOrange}`}>
                <Icon name="pause" size={13} weight={2.6} />
                Assistant paused
              </span>
            ) : null}
            {instance.state === 'degraded' ? (
              <span className={`${styles.chip} ${styles.chipOrange}`}>
                <Icon name="warning" size={13} weight={2.2} />
                Not responding
              </span>
            ) : null}
          </div>
        ) : null}
        {action && !overlay ? <div className={styles.action}>{action}</div> : null}
      </div>
    </div>
  );
}

function StateOverlay({ instance, size, onPower, powerBusy, hasFrame }: { instance: Instance; size: Props['size']; onPower?: Props['onPower']; powerBusy?: boolean; hasFrame: boolean }) {
  const info = instanceInfo(instance);
  const tiny = size === 'tiny';
  const locked = isLockedOff(instance);
  const loading = (instance.state === 'ready' || instance.state === 'degraded') && !hasFrame;
  const value = progressOf(instance);
  const progress = info.transitional && (value !== null || instance.state === 'encrypting');
  const mood = familiarFor(instance, loading, !!powerBusy);

  // The tiny glyph is too small for Familiar; it keeps the plain symbols.
  const glyph = locked ? (
    <span className={styles.lock}>
      <Icon name="lock" size={tiny ? 13 : size === 'hero' ? 34 : 22} weight={tiny ? 1.9 : 1.7} />
    </span>
  ) : tiny ? (
    loading || instance.state === 'stopping' ? (
      <Spinner size={12} />
    ) : progress ? null : instance.state === 'sleeping' || instance.state === 'hibernated' ? (
      <Icon name="moon" size={14} weight={1.7} />
    ) : instance.state === 'error' ? (
      <span className={styles.red}>
        <Icon name="warning" size={14} weight={1.8} />
      </span>
    ) : (
      <Icon name="power" size={14} weight={1.8} />
    )
  ) : (
    // One Familiar for every state, so Off to Starting to Ready is a single continuous performance.
    <Familiar state={mood} size={size === 'hero' ? 64 : 36} variant="glyph" />
  );

  return (
    <div className={styles.overlay}>
      {glyph ? <span className={styles.glyph}>{glyph}</span> : null}
      {loading ? (
        size === 'hero' ? <p className={styles.caption}>Getting the latest picture</p> : null
      ) : progress ? (
        <>
          <div className={styles.boot}>
            <ProgressBar value={value} label={info.label} tone="light" />
          </div>
          {size === 'hero' ? <p className={styles.caption}>{stateDetail(instance)}</p> : null}
        </>
      ) : size === 'hero' ? (
        <>
          <p className={styles.title}>{info.label}</p>
          <p className={styles.caption}>{stateDetail(instance)}</p>
          {onPower && locked ? (
            <div className={styles.overlayAction}>
              <Button variant="on-dark" onClick={() => void unlockMacs()}>
                Unlock…
              </Button>
            </div>
          ) : onPower && info.primary ? (
            <div className={styles.overlayAction}>
              <Button variant="on-dark" onClick={() => onPower(info.primary!.action)} busy={powerBusy}>
                {info.primary.label}
              </Button>
            </div>
          ) : null}
        </>
      ) : null}
    </div>
  );
}

/** Off and asleep: sleeping. Asked to start: waking. On its way up: thinking. Stopped unexpectedly: sad. */
function familiarFor(instance: Instance, loading: boolean, powerBusy: boolean): FamiliarState {
  if (loading) return 'thinking';
  switch (instance.state) {
    case 'starting':
    case 'booting':
    case 'encrypting':
      return 'thinking';
    case 'error':
      return powerBusy ? 'waking' : 'sad';
    case 'stopped':
    case 'sleeping':
    case 'hibernated':
      return powerBusy ? 'waking' : 'sleeping';
    case 'stopping':
      return 'sleeping';
    default:
      return 'idle';
  }
}
