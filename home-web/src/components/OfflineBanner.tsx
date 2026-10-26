import { checkNow, reachability } from '../lib/connection';
import { useSession } from '../lib/session';
import { useStore } from '../lib/store';
import { restart } from '../lib/updates';
import { Button } from './Button';
import { Icon } from './Icon';
import styles from './OfflineBanner.module.css';

/** A calm note at the top while the server can't be reached; it retries on its own. */
export function OfflineBanner() {
  const { online, checking } = useStore(reachability);
  // A restart for an update is expected; the Software Update page explains it.
  const restarting = useStore(restart).phase === 'waiting';
  // Before anything has loaded the whole page says it can't connect, so the banner would repeat it.
  const loaded = !!useSession().data;
  if (online || restarting || !loaded) return null;
  return (
    <div className={styles.banner} role="status">
      <Icon name="wifi" size={18} weight={2} />
      <p className={styles.text}>
        <strong>Can’t reach Familiar.</strong> <span>Showing what was loaded; changes wait until it’s back.</span>
      </p>
      <Button variant="gray" size="small" onClick={() => void checkNow()} busy={checking}>
        Try Again
      </Button>
    </div>
  );
}
