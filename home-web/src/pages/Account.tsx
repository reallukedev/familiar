import { useMemo, useState, type FormEvent } from 'react';
import { Avatar, Banner, Pill } from '../components/Bits';
import { Button } from '../components/Button';
import { Confirm } from '../components/Confirm';
import { TotpSetup } from '../components/TotpSetup';
import { Fields, FormError, TextField } from '../components/Field';
import { Icon, type IconName } from '../components/Icon';
import { CellBody, CellText, GlyphTile, Row, Section } from '../components/List';
import { Sheet } from '../components/Sheet';
import { SkeletonRows } from '../components/Skeleton';
import { ApiError, api } from '../lib/api';
import { keys, setData, updateData, useQuery } from '../lib/cache';
import { deviceName, newDeviceSignIns } from '../lib/device';
import { stamp, when, whenInline } from '../lib/format';
import { refreshSession, signOut, useSession } from '../lib/session';
import { openSettings } from '../lib/settingsWindow';
import { showNewRecoveryKey } from '../lib/recovery';
import { notify } from '../lib/toast';
import type { AccountInfo, SecurityEvent, User } from '../lib/types';
import { passwordError } from '../lib/validate';
import { Page } from '../shells/Page';
import styles from './Account.module.css';
import { withUndo } from '../lib/undo';

function adoptUser(user: User) {
  updateData<AccountInfo>(keys.account, (data) => ({ ...data, user }));
  void refreshSession().catch(() => {});
}

export function Account() {
  const { data: session } = useSession();
  const account = useQuery<AccountInfo>(keys.account, api.account);
  const security = useQuery<SecurityEvent[]>(keys.security, api.security);
  const user = account.data?.user ?? session?.user ?? null;
  const [name, setName] = useState<string | null>(null);
  const [savingName, setSavingName] = useState(false);
  const [sheet, setSheet] = useState<'password' | 'totp-on' | 'totp-change' | null>(null);
  const [signingOut, setSigningOut] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [makingKey, setMakingKey] = useState(false);
  const now = Date.now();

  async function newRecoveryKey() {
    setMakingKey(true);
    try {
      await showNewRecoveryKey();
    } catch (error) {
      notify.error(error);
    } finally {
      setMakingKey(false);
    }
  }

  if (!user) return <Page title="Account"><SkeletonRows rows={4} /></Page>;
  const editing = name ?? user.display_name;

  async function saveName(event: FormEvent) {
    event.preventDefault();
    if (!editing.trim() || editing.trim() === user!.display_name) return;
    setSavingName(true);
    try {
      adoptUser(await api.updateAccount({ display_name: editing.trim() }));
      setName(null);
      notify.success('Name saved');
    } catch (error) {
      notify.error(error);
    } finally {
      setSavingName(false);
    }
  }

  async function revokeSession(id: string) {
    try {
      await api.revokeSession(id);
      updateData<AccountInfo>(keys.account, (data) => ({ ...data, sessions: data.sessions.filter((item) => item.id !== id) }));
      notify.success('Signed out that device');
    } catch (error) {
      notify.error(error);
    }
  }

  const mustEnroll = !!session?.require_totp && !user.totp_enabled;

  return (
    <Page title="Account">
      <div className={styles.profile}>
        <Avatar name={user.display_name} size="large" />
        <div className={styles.profileText}>
          <h2 className="t-title2">{user.display_name}</h2>
          <p className="t-subhead secondary">
            {user.username} · {user.role === 'admin' ? 'Administrator' : 'Member'}
          </p>
        </div>
      </div>

      {mustEnroll ? (
        <div className={styles.banner}>
          <Banner tone="orange" icon="shield" title="Turn on two‑factor authentication" action={<Button variant="filled" onClick={() => setSheet('totp-on')}>Turn On</Button>}>
            This server requires it for every account.
          </Banner>
        </div>
      ) : null}

      <Section>
        <Row
          leading={
            <GlyphTile color="gray">
              <Icon name="gear" size={16} weight={2} />
            </GlyphTile>
          }
          title="Settings"
          onClick={() => openSettings()}
        />
      </Section>

      <Section title="Name">
        <CellBody>
          <form className={styles.inline} onSubmit={saveName}>
            <div className={styles.grow}>
              <TextField label="Display name" labelHidden value={editing} onValueChange={setName} maxLength={64} autoComplete="name" />
            </div>
            <Button type="submit" variant="tinted" disabled={!editing.trim() || editing.trim() === user.display_name} busy={savingName}>
              Save
            </Button>
          </form>
        </CellBody>
      </Section>

      <Section title="Sign-in and security">
        <Row title="Password" onClick={() => setSheet('password')} leading={<GlyphTile color="gray"><Icon name="key" size={17} weight={2} /></GlyphTile>} value="Change" chevron />
        <Row
          title="Two-factor authentication"
          subtitle={user.totp_enabled ? 'On' : 'Off'}
          leading={<GlyphTile color={user.totp_enabled ? 'green' : 'gray'}><Icon name="shield" size={17} weight={2} /></GlyphTile>}
          trailing={
            user.totp_enabled ? (
              <Button variant="plain" size="small" onClick={() => setSheet('totp-change')}>
                Change Code
              </Button>
            ) : (
              <Button variant="tinted" size="small" onClick={() => setSheet('totp-on')}>
                Turn On
              </Button>
            )
          }
        />
        <Row
          title="Recovery Key…"
          subtitle={session?.recovery_key_needed ? 'Not saved yet' : 'Make a new recovery key. Your old one stops working.'}
          leading={<GlyphTile color={session?.recovery_key_needed ? 'orange' : 'gray'}><Icon name="lock" size={16} weight={2.1} /></GlyphTile>}
          onClick={() => void newRecoveryKey()}
          disabled={makingKey}
          chevron
        />
      </Section>

      <Section title="Where you’re signed in" footer="Changing your password signs out every other device.">
        {account.data ? (
          account.data.sessions.map((item) => (
            <Row
              key={item.id}
              title={deviceName(item.user_agent)}
              subtitle={item.current ? 'Active now' : `Last active ${whenInline(item.last_seen_at, now)}`}
              trailing={
                item.current ? (
                  <Pill tone="green">This browser</Pill>
                ) : (
                  <Button variant="plain" size="small" onClick={() => setRevoking(item.id)}>
                    Sign Out
                  </Button>
                )
              }
            />
          ))
        ) : account.error ? (
          <CellText>{account.error.message}</CellText>
        ) : (
          <SkeletonRows rows={2} />
        )}
      </Section>

      <SecurityLog events={security.data} error={security.error?.message} now={now} />

      <div className={styles.danger}>
        <Section>
          <Row title="Sign Out" tone="destructive" onClick={() => setSigningOut(true)} chevron={false} />
        </Section>
      </div>

      <PasswordSheet open={sheet === 'password'} onOpenChange={(open) => setSheet(open ? 'password' : null)} />
      <TotpOnSheet open={sheet === 'totp-on'} onOpenChange={(open) => setSheet(open ? 'totp-on' : null)} />
      <TotpChangeSheet open={sheet === 'totp-change'} onOpenChange={(open) => setSheet(open ? 'totp-change' : null)} />
      <Confirm open={signingOut} onOpenChange={setSigningOut} title="Sign out of Familiar?" message="Your Macs keep running, and connected assistants keep working." action="Sign Out" onConfirm={() => void signOut()} />
      <Confirm open={!!revoking} onOpenChange={(open) => !open && setRevoking(null)} title="Sign out this device?" message="It has to sign in again to use Familiar." action="Sign Out" onConfirm={() => revoking && withUndo('Signing out that device…', () => revokeSession(revoking))} />
    </Page>
  );
}

const EVENT: Record<SecurityEvent['kind'], { title: string; icon: IconName; color: 'gray' | 'green' | 'orange' | 'red' | 'blue' }> = {
  sign_in: { title: 'Signed in', icon: 'person', color: 'blue' },
  sign_in_failed: { title: 'Sign-in failed', icon: 'xmark', color: 'red' },
  sign_out: { title: 'Signed out', icon: 'signout', color: 'gray' },
  password_changed: { title: 'Password changed', icon: 'key', color: 'orange' },
  totp_enabled: { title: 'Two-factor turned on', icon: 'shield', color: 'green' },
  totp_disabled: { title: 'Two-factor turned off', icon: 'shield', color: 'orange' },
  totp_changed: { title: 'Two-factor code changed', icon: 'shield', color: 'green' },
  token_created: { title: 'Access token created', icon: 'key', color: 'gray' },
  token_revoked: { title: 'Access token revoked', icon: 'key', color: 'gray' },
  grant_created: { title: 'Assistant connected', icon: 'link', color: 'green' },
  grant_revoked: { title: 'Assistant disconnected', icon: 'link', color: 'gray' },
  session_revoked: { title: 'Device signed out', icon: 'signout', color: 'gray' },
  locked_out: { title: 'Locked after failed sign-ins', icon: 'lock', color: 'red' },
  recovery_key_created: { title: 'New recovery key made', icon: 'lock', color: 'gray' },
  recovery_key_used: { title: 'Recovery key used', icon: 'lock', color: 'orange' },
  keys_reset: { title: 'Encrypted Macs erased', icon: 'lock', color: 'red' },
};

function SecurityLog({ events, error, now }: { events: SecurityEvent[] | undefined; error?: string; now: number }) {
  const fresh = useMemo(() => newDeviceSignIns(events ?? []), [events]);
  return (
    <Section title="Security log">
      {events ? (
        events.length ? (
          events.slice(0, 40).map((event) => {
            const kind = EVENT[event.kind];
            return (
              <Row
                key={event.id}
                align="top"
                leading={<GlyphTile color={kind.color}><Icon name={kind.icon} size={15} weight={2.2} /></GlyphTile>}
                title={
                  <>
                    {kind.title}
                    {fresh.has(event.id) ? (
                      <>
                        {' '}
                        <Pill tone="orange">New device</Pill>
                      </>
                    ) : null}
                  </>
                }
                subtitle={[event.detail, deviceName(event.user_agent), event.ip].filter(Boolean).join(' · ')}
                value={<span title={stamp(event.at)}>{when(event.at, now)}</span>}
              />
            );
          })
        ) : (
          <CellText>Nothing yet.</CellText>
        )
      ) : error ? (
        <CellText>{error}</CellText>
      ) : (
        <SkeletonRows rows={3} leading />
      )}
    </Section>
  );
}

function PasswordSheet({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [confirm, setConfirm] = useState('');
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const errors = { current: current ? null : 'Enter your current password.', next: passwordError(next), confirm: confirm === next ? null : 'The passwords don’t match.' };

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitted(true);
    if (Object.values(errors).some(Boolean)) return;
    setBusy(true);
    setError(null);
    try {
      await api.changePassword({ current, password: next });
      notify.success('Password changed. Other devices are signed out.');
      onOpenChange(false);
      setCurrent('');
      setNext('');
      setConfirm('');
      setSubmitted(false);
      void api.account().then((data) => setData(keys.account, data)).catch(() => {});
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.silent)) setError(cause instanceof ApiError ? cause.message : 'Couldn’t change the password. Try again.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Change Password" primary={{ label: 'Change Password', form: 'password-form', busy }}>
      <form id="password-form" className={styles.sheetForm} onSubmit={submit} noValidate>
        <input type="text" name="username" autoComplete="username" hidden readOnly />
        <Fields>
          <TextField label="Current password" type="password" value={current} onValueChange={setCurrent} autoComplete="current-password" error={submitted ? errors.current : null} />
          <TextField label="New password" type="password" value={next} onValueChange={setNext} autoComplete="new-password" error={submitted ? errors.next : null} description="At least 10 characters." />
          <TextField label="Confirm new password" type="password" value={confirm} onValueChange={setConfirm} autoComplete="new-password" error={submitted ? errors.confirm : null} />
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}

function TotpOnSheet({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Turn On Two-Factor" dismissLabel="Cancel">
      <div className={styles.sheetForm}>
        {open ? (
          <TotpSetup
            onEnrolled={(user) => {
              adoptUser(user);
              notify.success('Two-factor authentication is on');
              onOpenChange(false);
            }}
          />
        ) : null}
      </div>
    </Sheet>
  );
}

/**
 * Moves two-factor to a new authenticator. The old code keeps working until a
 * code from the new one is confirmed, so the account is never without it.
 */
function TotpChangeSheet({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Change Two-Factor Code" dismissLabel="Cancel" description="Your current code stops working once you confirm the new one.">
      <div className={styles.sheetForm}>
        {open ? (
          <TotpSetup
            confirmLabel="Change Code"
            onEnrolled={(user) => {
              adoptUser(user);
              notify.success('Two-factor code changed');
              onOpenChange(false);
            }}
          />
        ) : null}
      </div>
    </Sheet>
  );
}
