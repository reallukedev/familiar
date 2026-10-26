import { useEffect, useState, type FormEvent } from 'react';
import type { FamiliarState } from '../components/Familiar';
import { Button } from '../components/Button';
import { CodeField, Fields, FormError, TextField } from '../components/Field';
import { ApiError, api } from '../lib/api';
import { countdown } from '../lib/format';
import { celebrate } from '../lib/celebrate';
import { navigate, useSearchParam } from '../lib/router';
import { safeNext } from '../lib/routes';
import { adoptSession } from '../lib/session';
import type { SessionInfo } from '../lib/types';
import { AuthLayout } from './AuthLayout';
import styles from './Forms.module.css';

function describe(error: unknown): string {
  return error instanceof ApiError ? error.message : 'Couldn’t sign in. Try again.';
}


/** Seconds until a rate limit lifts, ticking once a second. */
function useLockout() {
  const [until, setUntil] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!until) return;
    const timer = setInterval(() => {
      const t = Date.now();
      setNow(t);
      if (t >= until) setUntil(null);
    }, 1000);
    return () => clearInterval(timer);
  }, [until]);
  const left = until ? Math.max(0, (until - now) / 1000) : 0;
  return {
    left,
    locked: left > 0,
    catchLimit(error: unknown) {
      if (error instanceof ApiError && error.code === 'rate_limited' && error.retryAfterS) {
        setNow(Date.now());
        setUntil(Date.now() + error.retryAfterS * 1000);
      }
    },
  };
}

export function SignIn() {
  const next = safeNext(useSearchParam('next'));
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [challenge, setChallenge] = useState<string | null>(null);
  const [code, setCode] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Familiar's face: sad after a wrong answer until you change it, happy on the way in.
  const [mood, setMood] = useState<FamiliarState>('idle');
  const lockout = useLockout();
  const face: FamiliarState = mood === 'happy' ? 'happy' : busy ? 'thinking' : mood;

  async function finish(session: SessionInfo) {
    setMood('happy');
    await celebrate();
    adoptSession(session);
    navigate(next, { replace: true });
  }

  function failed() {
    setMood('sad');
  }

  async function submitPassword(event: FormEvent) {
    event.preventDefault();
    if (!username || !password) {
      setError('Enter your username and password.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await finish(await api.login({ username: username.trim().toLowerCase(), password }));
    } catch (cause) {
      if (cause instanceof ApiError && cause.code === 'totp_required' && typeof cause.extra.challenge === 'string') {
        setChallenge(cause.extra.challenge);
        setPassword('');
        setMood('idle');
      } else {
        failed();
        setError(describe(cause));
        lockout.catchLimit(cause);
      }
    } finally {
      setBusy(false);
    }
  }

  async function submitCode(value = code) {
    if (!challenge || value.length !== 6) return;
    setBusy(true);
    setError(null);
    try {
      await finish(await api.loginTotp({ challenge, code: value }));
    } catch (cause) {
      failed();
      setError(describe(cause));
      setCode('');
      lockout.catchLimit(cause);
      if (cause instanceof ApiError && (cause.extra.restart === 'true' || cause.extra.restart === true || cause.code === 'unauthenticated')) {
        // The challenge expired or ran out of tries: start over from the password.
        setChallenge(null);
      }
    } finally {
      setBusy(false);
    }
  }

  if (challenge) {
    return (
      <AuthLayout mood={face} title="Enter your code" footer="Lost your authenticator? Ask the person who runs this server to reset it on the server.">
        <p className={`t-body secondary ${styles.lead}`}>
          Open your authenticator app and enter the six-digit code for Familiar.
        </p>
        <form
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            void submitCode();
          }}
        >
          <CodeField value={code} onValueChange={(value) => { setCode(value); if (mood === 'sad') setMood('idle'); }} onComplete={(value) => void submitCode(value)} invalid={!!error} disabled={busy} />
          <FormError>{error && lockout.locked ? `${error} Try again in ${countdown(lockout.left)}.` : error}</FormError>
          <Button type="submit" variant="filled" size="large" width="full" busy={busy} disabled={code.length !== 6 || lockout.locked}>
            Verify
          </Button>
          <Button
            variant="plain"
            width="full"
            onClick={() => {
              setChallenge(null);
              setError(null);
              setCode('');
            }}
          >
            Use a Different Account
          </Button>
        </form>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout mood={face} title="Sign in to Familiar" footer="Forgot your password? Ask the person who runs this server for a reset link.">
      <form className={styles.form} onSubmit={submitPassword} noValidate>
        <Fields>
          <TextField label="Username" value={username} onValueChange={(value) => { setUsername(value); if (mood === 'sad') setMood('idle'); }} autoComplete="username" autoCapitalize="none" autoCorrect="off" spellCheck={false} name="username" required />
          <TextField label="Password" value={password} onValueChange={(value) => { setPassword(value); if (mood === 'sad') setMood('idle'); }} type="password" autoComplete="current-password" name="password" required />
        </Fields>
        <FormError>{error && lockout.locked ? `${error} Try again in ${countdown(lockout.left)}.` : error}</FormError>
        <Button type="submit" variant="filled" size="large" width="full" busy={busy} disabled={lockout.locked}>
          Sign In
        </Button>
      </form>
    </AuthLayout>
  );
}
