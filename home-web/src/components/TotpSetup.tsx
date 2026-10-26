import { useEffect, useMemo, useState } from 'react';
import { renderSVG } from 'uqr';
import { ApiError, api } from '../lib/api';
import { groupSecret } from '../lib/format';
import type { User } from '../lib/types';
import { Button } from './Button';
import { CopyButton } from './Copy';
import { CodeField, FormError } from './Field';
import { SkeletonBlock } from './Skeleton';
import styles from './TotpSetup.module.css';

/**
 * Two-factor enrollment: scan the QR code (or type the key), then confirm a
 * code. Each mount asks for a fresh secret; it isn't active until confirmed.
 */
export function TotpSetup({ onEnrolled, confirmLabel = 'Turn On' }: { onEnrolled: (user: User) => void; confirmLabel?: string }) {
  const [enrollment, setEnrollment] = useState<{ secret: string; otpauth_url: string } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [code, setCode] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    api
      .startTotp()
      .then((result) => !cancelled && setEnrollment(result))
      .catch((cause: unknown) => !cancelled && setLoadError(cause instanceof ApiError ? cause.message : 'Couldn’t start setup. Try again.'));
    return () => {
      cancelled = true;
    };
  }, [attempt]);

  const qr = useMemo(() => (enrollment ? `data:image/svg+xml;utf8,${encodeURIComponent(renderSVG(enrollment.otpauth_url, { ecc: 'M', pixelSize: 6 }))}` : null), [enrollment]);

  async function confirm(value = code) {
    if (value.length !== 6 || busy) return;
    setBusy(true);
    setError(null);
    try {
      onEnrolled(await api.confirmTotp(value));
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : 'That code didn’t work. Try the newest one.');
      setCode('');
    } finally {
      setBusy(false);
    }
  }

  if (loadError) {
    return (
      <div className={styles.setup}>
        <FormError>{loadError}</FormError>
        <Button variant="tinted" width="full" onClick={() => setAttempt((n) => n + 1)}>
          Try Again
        </Button>
      </div>
    );
  }
  if (!enrollment || !qr) {
    return (
      <div className={styles.setup} role="status" aria-label="Preparing two-factor setup">
        <SkeletonBlock height={200} />
        <SkeletonBlock height={56} />
      </div>
    );
  }

  return (
    <form
      className={styles.setup}
      onSubmit={(event) => {
        event.preventDefault();
        void confirm();
      }}
    >
      <ol className={styles.steps}>
        <li>
          <p>Scan this code with an authenticator app, such as Passwords on iPhone or Mac.</p>
          <img className={styles.qr} src={qr} alt="QR code to add Familiar to your authenticator app" width={176} height={176} />
          <div className={styles.secret}>
            <span className="t-footnote secondary">Can’t scan it? Enter this key:</span>
            <span className={styles.secretRow}>
              <code className={styles.secretCode}>{groupSecret(enrollment.secret)}</code>
              <CopyButton text={enrollment.secret} what="setup key" />
            </span>
          </div>
        </li>
        <li>
          <p>Enter the six-digit code the app shows for Familiar.</p>
          <CodeField value={code} onValueChange={setCode} onComplete={(value) => void confirm(value)} invalid={!!error} disabled={busy} />
        </li>
      </ol>
      <FormError>{error}</FormError>
      <Button type="submit" variant="filled" size="large" width="full" busy={busy} disabled={code.length !== 6}>
        {confirmLabel}
      </Button>
    </form>
  );
}
