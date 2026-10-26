import { useState, type FormEvent } from 'react';
import type { FamiliarState } from '../components/Familiar';
import { Button } from '../components/Button';
import { Fields, FormError, TextField } from '../components/Field';
import { ApiError, api } from '../lib/api';
import { celebrate } from '../lib/celebrate';
import { navigate, useSearchParam } from '../lib/router';
import { adoptSession } from '../lib/session';
import { passwordError, usernameError } from '../lib/validate';
import { AuthLayout } from './AuthLayout';
import styles from './Forms.module.css';

/** First run: whoever opens the setup link printed by the server becomes the admin. */
export function Setup() {
  const token = useSearchParam('token');
  const [username, setUsername] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fieldError, setFieldError] = useState<{ field: string; message: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [mood, setMood] = useState<FamiliarState>('idle');

  if (!token) {
    return (
      <AuthLayout title="Set up Familiar">
        <p className={`t-body secondary ${styles.lead}`}>
          Open the setup link the server printed when it first started. It looks like https://your-server/setup?token=… and works once.
        </p>
      </AuthLayout>
    );
  }

  const errors = {
    username: usernameError(username.trim()),
    display_name: displayName.trim() ? null : 'Add the name people will see.',
    password: passwordError(password),
    confirm: confirm === password ? null : 'The passwords don’t match.',
  };
  const show = (key: keyof typeof errors) => (fieldError?.field === key ? fieldError.message : submitted ? errors[key] : null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitted(true);
    setFieldError(null);
    if (Object.values(errors).some(Boolean)) return;
    setBusy(true);
    setError(null);
    try {
      const session = await api.setup({ token: token!, username: username.trim(), display_name: displayName.trim(), password });
      setMood('happy');
      await celebrate();
      adoptSession(session);
      navigate('/', { replace: true });
    } catch (cause) {
      setMood('sad');
      if (cause instanceof ApiError && cause.field) setFieldError({ field: cause.field, message: cause.message });
      else setError(cause instanceof ApiError ? cause.message : 'Couldn’t finish setup. Try again.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <AuthLayout mood={mood === 'happy' ? 'happy' : busy ? 'thinking' : mood} title="Set up Familiar" footer="This account runs the server: it invites people, sets limits and builds the macOS image. You can add two-factor authentication right after.">
      <p className={`t-body secondary ${styles.lead}`}>Create the administrator account.</p>
      <form className={styles.form} onSubmit={submit} noValidate>
        <Fields>
          <TextField label="Your name" value={displayName} onValueChange={setDisplayName} autoComplete="name" error={show('display_name')} />
          <TextField label="Username" value={username} onValueChange={(value) => setUsername(value.toLowerCase())} autoComplete="username" autoCapitalize="none" spellCheck={false} error={show('username')} description="Lowercase letters, numbers, dots, dashes and underscores." />
          <TextField label="Password" type="password" value={password} onValueChange={setPassword} autoComplete="new-password" error={show('password')} description="At least 10 characters. A passphrase of a few words works well." />
          <TextField label="Confirm password" type="password" value={confirm} onValueChange={setConfirm} autoComplete="new-password" error={show('confirm')} />
        </Fields>
        <FormError>{error}</FormError>
        <Button type="submit" variant="filled" size="large" width="full" busy={busy}>
          Create Admin Account
        </Button>
      </form>
    </AuthLayout>
  );
}
