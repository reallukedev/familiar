import { useEffect, useState, type FormEvent } from 'react';
import { ApiError, api } from '../lib/api';
import { reauthRequest, sessionEnded } from '../lib/reauth';
import { useSession } from '../lib/session';
import { useStore } from '../lib/store';
import { CodeField, Fields, FormError, TextField } from './Field';
import { Sheet } from './Sheet';
import styles from './ReauthSheet.module.css';

/**
 * Asked for before sensitive changes (tokens, deleting a Mac, security settings)
 * when the password hasn't been confirmed in the last ten minutes. The original
 * request continues on its own once this succeeds.
 */
export function ReauthSheet() {
  const pending = useStore(reauthRequest);
  const { data: session } = useSession();
  const totp = !!session?.user?.totp_enabled;
  // Kept while the sheet animates away, so its title doesn't change on the way out.
  const [unlocking, setUnlocking] = useState(false);
  if (pending && (pending.reason === 'unlock') !== unlocking) setUnlocking(pending.reason === 'unlock');
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Any request finding the session gone sends the person to sign in; don't leave this open over it.
  const signedOut = !!session && !session.user;
  useEffect(() => {
    if (pending && signedOut) pending.resolve(false);
  }, [pending, signedOut]);

  useEffect(() => {
    if (!pending) {
      setPassword('');
      setCode('');
      setError(null);
    }
  }, [pending]);

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    if (!pending) return;
    if (!password) {
      setError('Enter your password.');
      return;
    }
    if (totp && code.length !== 6) {
      setError('Enter the six-digit code from your authenticator app.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.confirm({ password, ...(totp ? { code } : {}) });
      pending.resolve(true);
    } catch (cause) {
      if (cause instanceof ApiError && cause.code === 'unauthenticated') {
        // Signed out meanwhile (the session expired): a password here can't help.
        sessionEnded();
        return;
      }
      setError(cause instanceof ApiError ? cause.message : 'Couldn’t confirm. Try again.');
      setCode('');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={!!pending}
      onOpenChange={(open) => {
        if (!open) pending?.resolve(false);
      }}
      title={unlocking ? 'Unlock Your Macs' : 'Confirm It’s You'}
      description={
        unlocking
          ? 'Familiar restarted, so your encrypted Macs are locked. Enter your password to unlock them.'
          : 'This change needs your password. You won’t be asked again for ten minutes.'
      }
      primary={{ label: unlocking ? 'Unlock' : 'Continue', form: 'reauth', busy }}
    >
      <form id="reauth" className={styles.form} onSubmit={submit} noValidate>
        <input type="text" name="username" autoComplete="username" value={session?.user?.username ?? ''} hidden readOnly />
        <Fields>
          <TextField label="Password" type="password" value={password} onValueChange={setPassword} autoComplete="current-password" />
          {totp ? (
            <div className={styles.code}>
              <span className="t-footnote secondary">Code from your authenticator app</span>
              <CodeField value={code} onValueChange={setCode} invalid={!!error && code.length === 0 && !!password} disabled={busy} />
            </div>
          ) : null}
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}
