import { useId, useState, type FormEvent } from 'react';
import { Banner } from '../components/Bits';
import { Button } from '../components/Button';
import { CodeField, Fields, FormError, TextField } from '../components/Field';
import { SkeletonBlock } from '../components/Skeleton';
import { ApiError, api } from '../lib/api';
import { useQuery } from '../lib/cache';
import { count, day } from '../lib/format';
import { formatRecoveryKey, recoveryKeyError } from '../lib/recoveryKey';
import { navigate } from '../lib/router';
import { adoptSession } from '../lib/session';
import { notify } from '../lib/toast';
import { passwordError, usernameError } from '../lib/validate';
import { AuthLayout } from './AuthLayout';
import styles from './Forms.module.css';

/** The reset went through but the code step can't finish here: sign in with the new password. */
function signInInstead() {
  notify.success('Password changed. Sign in with it.');
  navigate('/signin', { replace: true });
}

/** Joining from an invite link, or choosing a new password from a reset link. */
export function Invite({ code }: { code: string }) {
  const invite = useQuery(`invite:${code}`, () => api.invite(code), { staleMs: 60_000 });
  const [username, setUsername] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [recoveryKey, setRecoveryKey] = useState('');
  // Reset links for accounts with encrypted Macs: keep them with the recovery key, or erase them.
  const [erase, setErase] = useState(false);
  const [eraseConfirmed, setEraseConfirmed] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Accounts with two-factor finish a reset with their code, here on this page:
  // the server holds their data key (and any new recovery key) under this challenge.
  const [challenge, setChallenge] = useState<string | null>(null);
  const [totpCode, setTotpCode] = useState('');
  const [codeError, setCodeError] = useState<string | null>(null);
  const eraseId = useId();

  // The code step comes first: the used link no longer loads once the password changed.
  if (challenge) {
    return (
      <AuthLayout title="Enter your code" footer="Lost your authenticator? Ask the person who runs this server to reset it on the server.">
        <p className={`t-body secondary ${styles.lead}`}>Your password is changed. Enter the six-digit code from your authenticator app to finish.</p>
        <form
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            void submitCode();
          }}
        >
          <CodeField
            value={totpCode}
            onValueChange={(value) => {
              setTotpCode(value);
              setCodeError(null);
            }}
            onComplete={(value) => void submitCode(value)}
            invalid={!!codeError}
            disabled={busy}
          />
          <FormError>{codeError}</FormError>
          <Button type="submit" variant="filled" size="large" width="full" busy={busy} disabled={totpCode.length !== 6}>
            Verify
          </Button>
        </form>
      </AuthLayout>
    );
  }

  if (invite.error) {
    const gone = invite.error.code === 'not_found';
    return (
      <AuthLayout title={gone ? 'This link no longer works' : 'Couldn’t open the invite'}>
        <p className={`t-body secondary ${styles.lead}`}>{gone ? 'It expired or was already used. Ask the person who sent it for a new one.' : invite.error.message}</p>
        {gone ? null : (
          <Button variant="tinted" width="full" onClick={() => void invite.reload()}>
            Try Again
          </Button>
        )}
      </AuthLayout>
    );
  }
  if (!invite.data) {
    return (
      <AuthLayout title="Join Familiar">
        <div role="status" aria-label="Loading invite" style={{ display: 'grid', gap: 16 }}>
          <SkeletonBlock height={50} />
          <SkeletonBlock height={50} />
          <SkeletonBlock height={50} />
        </div>
      </AuthLayout>
    );
  }

  const resetName = invite.data.reset_username ?? invite.data.username ?? null;
  const reset = !!resetName;
  const needsKey = reset && !!invite.data.needs_recovery_key;
  const macs = invite.data.encrypted_macs ?? 0;
  const name = reset ? resetName! : username.trim();
  const errors = {
    username: reset ? null : usernameError(name),
    display_name: reset || displayName.trim() ? null : 'Add the name people will see.',
    password: passwordError(password),
    recovery_key: needsKey && !erase ? recoveryKeyError(recoveryKey) : null,
    erase: needsKey && erase && !eraseConfirmed ? 'Confirm that your encrypted Macs will be erased.' : null,
  };

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitted(true);
    setKeyError(null);
    if (Object.values(errors).some(Boolean)) return;
    setBusy(true);
    setError(null);
    try {
      const session = await api.acceptInvite(code, {
        username: name,
        display_name: reset ? '' : displayName.trim(),
        password,
        ...(needsKey ? (erase ? { discard_encrypted_macs: true } : { recovery_key: formatRecoveryKey(recoveryKey) }) : {}),
      });
      adoptSession(session);
      navigate('/', { replace: true });
    } catch (cause) {
      if (cause instanceof ApiError && cause.code === 'recovery_key_required') {
        // The account gained an encrypted Mac since this page loaded.
        setErase(false);
        setKeyError(cause.message);
        void invite.reload();
      } else if (cause instanceof ApiError && cause.field === 'recovery_key') {
        setKeyError(cause.message);
      } else if (cause instanceof ApiError && cause.code === 'totp_required') {
        // The password changed; the authenticator code finishes signing in.
        if (typeof cause.extra.challenge === 'string') {
          setChallenge(cause.extra.challenge);
          setPassword('');
        } else {
          signInInstead();
        }
      } else {
        setError(cause instanceof ApiError ? cause.message : 'Couldn’t finish. Try again.');
      }
    } finally {
      setBusy(false);
    }
  }

  async function submitCode(value = totpCode) {
    if (!challenge || value.length !== 6) return;
    setBusy(true);
    setCodeError(null);
    try {
      // A new recovery key in this reply opens the Recovery Key sheet on its own.
      adoptSession(await api.loginTotp({ challenge, code: value }));
      navigate('/', { replace: true });
    } catch (cause) {
      setTotpCode('');
      if (cause instanceof ApiError && (cause.extra.restart === 'true' || cause.extra.restart === true || cause.code === 'unauthenticated')) {
        // The challenge expired or ran out of tries. The new password is set, so sign in with it.
        signInInstead();
      } else {
        setCodeError(cause instanceof ApiError ? cause.message : 'Couldn’t check the code. Try again.');
      }
    } finally {
      setBusy(false);
    }
  }

  const erasing = needsKey && erase;
  return (
    <AuthLayout
      title={reset ? 'Choose a new password' : 'Join Familiar'}
      footer={reset ? 'Signing in elsewhere ends once you change it.' : `This link works once and expires ${day(invite.data.expires_at)}.`}
    >
      <p className={`t-body secondary ${styles.lead}`}>
        {reset ? `For the account ${resetName}.` : invite.data.role === 'admin' ? 'You’re invited as an administrator. You’ll get your own private Mac.' : 'You’re invited to get your own private Mac that your assistant can use.'}
      </p>
      <form className={styles.form} onSubmit={submit} noValidate>
        <Fields>
          {reset ? (
            <input type="text" name="username" autoComplete="username" value={name} readOnly hidden />
          ) : (
            <>
              <TextField label="Your name" value={displayName} onValueChange={setDisplayName} autoComplete="name" error={submitted ? errors.display_name : null} />
              <TextField label="Username" value={username} onValueChange={(value) => setUsername(value.toLowerCase())} autoComplete="username" autoCapitalize="none" spellCheck={false} error={submitted ? errors.username : null} />
            </>
          )}
          <TextField label={reset ? 'New password' : 'Password'} type="password" value={password} onValueChange={setPassword} autoComplete="new-password" error={submitted ? errors.password : null} description="At least 10 characters." />
          {needsKey && !erase ? (
            <TextField
              label="Recovery key"
              value={recoveryKey}
              onValueChange={(value) => {
                setRecoveryKey(value);
                setKeyError(null);
              }}
              mono
              autoComplete="off"
              autoCapitalize="characters"
              spellCheck={false}
              placeholder="XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX"
              error={keyError ?? (submitted ? errors.recovery_key : null)}
              description={`Keeps the data on your ${count(macs, 'encrypted Mac')}. Dashes are optional.`}
            />
          ) : null}
        </Fields>

        {needsKey ? (
          erase ? (
            <div className={styles.erase}>
              <Banner tone="red" icon="warning" title="Your encrypted Macs will be erased">
                Without your recovery key, the data on your {count(macs, 'encrypted Mac')} can’t be recovered. {macs === 1 ? 'It’ll' : 'They’ll'} be erased.
              </Banner>
              <label className={styles.check} htmlFor={eraseId}>
                <input id={eraseId} type="checkbox" checked={eraseConfirmed} onChange={(event) => setEraseConfirmed(event.target.checked)} />
                <span className="t-body">Erase my encrypted Macs</span>
              </label>
              {submitted && errors.erase ? <p className="t-footnote text-red">{errors.erase}</p> : null}
              <button type="button" className={styles.link} onClick={() => setErase(false)}>
                Use My Recovery Key
              </button>
            </div>
          ) : (
            <button
              type="button"
              className={styles.link}
              onClick={() => {
                setErase(true);
                setKeyError(null);
              }}
            >
              Don’t Have It?
            </button>
          )
        ) : null}

        <FormError>{error}</FormError>
        <Button type="submit" variant={erasing ? 'destructive-filled' : 'filled'} size="large" width="full" busy={busy} disabled={erasing && !eraseConfirmed}>
          {erasing ? 'Erase and Reset Password' : reset ? 'Change Password' : 'Create Account'}
        </Button>
      </form>
    </AuthLayout>
  );
}
