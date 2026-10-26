import { useEffect, useId, useState } from 'react';
import { api } from '../lib/api';
import { downloadRecoveryKey, showNewRecoveryKey, shownRecoveryKey } from '../lib/recovery';
import { formatRecoveryKey } from '../lib/recoveryKey';
import { refreshSession, useSession } from '../lib/session';
import { useStore } from '../lib/store';
import { notify } from '../lib/toast';
import { Banner } from './Bits';
import { Button } from './Button';
import { CopyButton } from './Copy';
import { FormError } from './Field';
import { Icon } from './Icon';
import { Sheet } from './Sheet';
import styles from './RecoveryKeySheet.module.css';

/**
 * The recovery key, shown once: when an account is made, when an existing
 * account is upgraded, and when the person asks for a new one. It can't be
 * dismissed until they say they saved it.
 */
export function RecoveryKeySheet() {
  const key = useStore(shownRecoveryKey);
  const { data: session } = useSession();
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const checkId = useId();

  // A new key starts unticked.
  useEffect(() => {
    setSaved(false);
    setError(null);
  }, [key]);

  async function finish() {
    if (!saved) return;
    setBusy(true);
    setError(null);
    try {
      await api.recoveryKeySaved();
      shownRecoveryKey.set(null);
      await refreshSession().catch(() => {});
    } catch {
      setError('Couldn’t save that. Try again.');
    } finally {
      setBusy(false);
    }
  }

  const account = session?.user?.username ?? null;
  const origin = session?.public_url ?? location.origin;
  const groups = key ? formatRecoveryKey(key).split('-') : [];

  return (
    <Sheet
      open={!!key}
      onOpenChange={(open) => {
        // Closing counts only once they've said they saved it.
        if (!open && saved) void finish();
      }}
      dismissible={false}
      title="Your Recovery Key"
      description="If you forget your password, this key is the only way back into the data on your Macs."
      primary={{ label: 'Continue', onClick: () => void finish(), disabled: !saved, busy }}
    >
      {key ? (
        <div className={styles.body}>
          <div className={styles.key} translate="no" aria-label={`Recovery key ${groups.join(' ')}`}>
            {groups.map((group, index) => (
              <span key={index} className={styles.group}>
                {group}
              </span>
            ))}
          </div>
          <div className={styles.actions}>
            <CopyButton text={formatRecoveryKey(key)} what="recovery key" />
            <button type="button" className={styles.download} onClick={() => downloadRecoveryKey(key, account, origin)}>
              <Icon name="download" size={15} weight={2.1} />
              Download
            </button>
          </div>
          <p className="t-footnote secondary pretty">Keep it somewhere safe, away from this server, like a password manager or a printed copy. Familiar can’t show it again.</p>
          <label className={styles.check} htmlFor={checkId}>
            <input id={checkId} type="checkbox" checked={saved} onChange={(event) => setSaved(event.target.checked)} />
            <span className="t-body">I saved my recovery key</span>
          </label>
          <FormError>{error}</FormError>
        </div>
      ) : null}
    </Sheet>
  );
}

/**
 * The account has no saved recovery key and none is on screen (the page was
 * reloaded before Saved It, or an older account was just upgraded).
 */
export function RecoveryKeyPrompt() {
  const { data: session } = useSession();
  const key = useStore(shownRecoveryKey);
  const [busy, setBusy] = useState(false);
  if (!session?.user || !session.recovery_key_needed || key) return null;

  async function show() {
    setBusy(true);
    try {
      await showNewRecoveryKey();
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className={styles.prompt}>
      <Banner
        tone="blue"
        icon="key"
        title="Save Your Recovery Key"
        action={
          <Button variant="filled" size="small" busy={busy} onClick={() => void show()}>
            Show Recovery Key…
          </Button>
        }
      >
        It keeps your Macs’ data safe if you forget your password.
      </Banner>
    </div>
  );
}
