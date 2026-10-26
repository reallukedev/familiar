import { useState, type FormEvent } from 'react';
import { Avatar, Banner, EmptyState, Pill, StatusLabel } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { CopyField } from '../../components/Copy';
import { Fields, TextField } from '../../components/Field';
import { Icon } from '../../components/Icon';
import { CellBody, GlyphTile, Row, Section } from '../../components/List';
import { Meter, ProgressBar } from '../../components/Meter';
import { Picker } from '../../components/Picker';
import { Segmented } from '../../components/Segmented';
import { Sheet } from '../../components/Sheet';
import { SkeletonRows } from '../../components/Skeleton';
import { Switch } from '../../components/Switch';
import { api } from '../../lib/api';
import { keys, setData, updateData, useQuery } from '../../lib/cache';
import { bytes, count, day, memory, uptime, whenInline } from '../../lib/format';
import { useLive } from '../../lib/live';
import { navigate } from '../../lib/router';
import { type AdminSection } from '../../lib/routes';
import { useSession } from '../../lib/session';
import { isOn, stateInfo } from '../../lib/status';
import { notify } from '../../lib/toast';
import type { AdminInstance, AdminUser, HostOverview, ImageStatus, Invite, Role } from '../../lib/types';
import { Page } from '../../shells/Page';
import { ADMIN_NAV } from '../../shells/nav';
import styles from './Admin.module.css';
import { StorageAdmin } from './StorageAdmin';
import { SoftwareUpdate } from './Updates';
import { withUndo } from '../../lib/undo';

/** Server administration. Admins see who has a Mac and what it uses, never what's on it. */
export default function AdminRoute({ section }: { section: AdminSection }) {
  const { data: session } = useSession();
  if (session?.user?.role !== 'admin') {
    return (
      <Page title="Server">
        <EmptyState icon="lock" title="Only administrators can open this">Ask the person who runs this server if you need a change.</EmptyState>
      </Page>
    );
  }
  switch (section) {
    case 'overview':
      return <Overview />;
    case 'users':
      return <Users />;
    case 'invites':
      return <Invites />;
    case 'macs':
      return <AllMacs />;
    case 'storage':
      return <StorageAdmin />;
    case 'image':
      return <Image />;
    case 'updates':
      return <SoftwareUpdate />;
    case 'settings':
      return <Policies />;
  }
}

const back = { href: '/admin', label: 'Server' };

function Overview() {
  const overview = useQuery<HostOverview>(keys.overview, api.overview, { pollMs: 10_000 });
  const data = overview.data;
  const phoneNav = (
    <Section>
      {ADMIN_NAV.slice(1).map((item) => (
        <Row key={item.section} href={`/admin/${item.section}`} title={item.label} leading={<GlyphTile color={item.section === 'settings' ? 'blue' : item.section === 'image' ? 'indigo' : item.section === 'updates' ? 'green' : 'gray'}><Icon name={item.icon} size={17} weight={2} /></GlyphTile>} />
      ))}
    </Section>
  );
  return (
    <Page title="Server">
      {overview.error && !data ? (
        <EmptyState icon="wifi" title="Couldn’t load the server overview" action={<Button variant="tinted" onClick={() => void overview.reload()}>Try Again</Button>}>
          {overview.error.message}
        </EmptyState>
      ) : !data ? (
        <div className={styles.gap}>
          <SkeletonRows rows={4} />
        </div>
      ) : (
        <>
          <p className={`t-subhead secondary ${styles.host}`}>
            {data.hostname} · Familiar {data.version} · Up {uptime(data.uptime_s)}
          </p>
          {data.checks.some((check) => !check.ok) ? (
            <div className={styles.gap}>
              <Banner tone="red" icon="warning" title={`${count(data.checks.filter((check) => !check.ok).length, 'check')} need attention`}>
                See Health below for what to fix.
              </Banner>
            </div>
          ) : null}
          <div className={styles.only}>{phoneNav}</div>
          <Section title="Resources" footer={`${count(data.users, 'person', 'people')} and ${count(data.instances, 'Mac')} on this server.`}>
            <CellBody>
              <div className={styles.meters}>
                <Meter label="Processor load" value={data.load_1m} max={data.cpu_count} valueText={`${data.load_1m.toFixed(1)} of ${data.cpu_count} cores`} />
                <Meter label="Memory" value={data.memory_total_mb - data.memory_available_mb} max={data.memory_total_mb} valueText={`${memory(data.memory_total_mb - data.memory_available_mb)} of ${memory(data.memory_total_mb)} used`} />
                <Meter label="Disk" value={data.disk_total_bytes - data.disk_free_bytes} max={data.disk_total_bytes} valueText={`${bytes(data.disk_free_bytes)} free of ${bytes(data.disk_total_bytes)}`} />
              </div>
            </CellBody>
          </Section>
          <Section title="Budget for Macs" footer="Over budget, the least recently used idle Mac hibernates first.">
            <CellBody>
              <div className={styles.meters}>
                <Meter label="Memory for running Macs" value={data.budget.used_memory_mb} max={data.budget.memory_mb} valueText={`${memory(data.budget.used_memory_mb)} of ${memory(data.budget.memory_mb)}`} />
                <Meter label="Running Macs" value={data.budget.running} max={data.budget.max_running} valueText={`${data.budget.running} of ${data.budget.max_running}`} />
              </div>
            </CellBody>
          </Section>
          <Section title="Health">
            {data.checks.map((check) => (
              <Row
                key={check.id}
                align="top"
                leading={<GlyphTile color={check.ok ? 'green' : 'red'}><Icon name={check.ok ? 'check' : 'exclamation'} size={15} weight={2.4} /></GlyphTile>}
                title={check.label}
                subtitle={check.detail}
              />
            ))}
          </Section>
        </>
      )}
    </Page>
  );
}

function Users() {
  const users = useQuery<AdminUser[]>(keys.users, api.users);
  const { data: session } = useSession();
  const [selected, setSelected] = useState<AdminUser | null>(null);
  const current = selected ? (users.data?.find((user) => user.id === selected.id) ?? selected) : null;
  return (
    <Page title="People" back={back} actions={<Button variant="gray" onClick={() => navigate('/admin/invites')}>Invite Someone</Button>}>
      {users.error && !users.data ? (
        <EmptyState icon="wifi" title="Couldn’t load people">{users.error.message}</EmptyState>
      ) : !users.data ? (
        <div className={styles.gap}><SkeletonRows rows={3} leading /></div>
      ) : (
        <Section footer="Admins never see anyone’s screens, files or activity.">
          {users.data.map((user) => (
            <Row
              key={user.id}
              onClick={() => setSelected(user)}
              leading={<Avatar name={user.display_name} size="small" />}
              title={
                <>
                  {user.display_name}
                  {user.id === session?.user?.id ? <span className="secondary"> (you)</span> : null}
                </>
              }
              subtitle={`${user.username} · ${user.role === 'admin' ? 'Admin' : 'Member'} · ${count(user.instance_count, 'Mac')} of ${user.max_instances}${user.last_seen_at ? ` · Seen ${whenInline(user.last_seen_at)}` : ''}`}
              trailing={user.disabled ? <Pill tone="red">Can’t sign in</Pill> : user.totp_enabled ? undefined : <Pill tone="gray">No two-factor</Pill>}
              chevron
            />
          ))}
        </Section>
      )}
      <UserSheet user={current} self={current?.id === session?.user?.id} onClose={() => setSelected(null)} />
    </Page>
  );
}

function putUser(user: AdminUser) {
  updateData<AdminUser[]>(keys.users, (list) => list.map((item) => (item.id === user.id ? user : item)));
}

function UserSheet({ user, self, onClose }: { user: AdminUser | null; self: boolean; onClose: () => void }) {
  const [resetUrl, setResetUrl] = useState<string | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [typed, setTyped] = useState('');

  async function patch(body: Parameters<typeof api.updateUser>[1], message: string) {
    if (!user) return;
    try {
      putUser(await api.updateUser(user.id, body));
      notify.success(message);
    } catch (cause) {
      notify.error(cause);
    }
  }
  async function reset() {
    if (!user) return;
    try {
      setResetUrl((await api.resetUser(user.id)).invite_url);
    } catch (cause) {
      notify.error(cause);
    }
  }
  async function remove(event: FormEvent) {
    event.preventDefault();
    if (!user || typed.trim() !== user.username) return;
    const target = user;
    setDeleting(false);
    onClose();
    withUndo(`Deleting ${target.display_name}…`, async () => {
      try {
        await api.deleteUser(target.id, target.username);
        updateData<AdminUser[]>(keys.users, (list) => list.filter((item) => item.id !== target.id));
        notify.success(`Deleted ${target.display_name}`);
      } catch (cause) {
        notify.error(cause);
      }
    });
  }

  return (
    <>
      <Sheet
        open={!!user && !deleting}
        onOpenChange={(open) => {
          if (!open) {
            onClose();
            setResetUrl(null);
          }
        }}
        title={user?.display_name ?? ''}
        description={user ? `${user.username} · Joined ${day(user.created_at)}` : undefined}
      >
        {user ? (
          <div className={styles.sheet}>
            {self ? <p className="t-footnote secondary">You can’t change your own role or sign-in here, so the server always has an admin.</p> : null}
            <div className={styles.field}>
              <span className="t-footnote secondary">Role</span>
              <Segmented label="Role" value={user.role} disabled={self} onChange={(role: Role) => void patch({ role }, role === 'admin' ? `${user.display_name} is now an admin` : `${user.display_name} is now a member`)} options={[{ value: 'member', label: 'Member' }, { value: 'admin', label: 'Admin' }]} />
            </div>
            <Section>
              <Row title="Can sign in" trailing={<Switch checked={!user.disabled} disabled={self} onCheckedChange={(on) => void patch({ disabled: !on }, on ? 'Sign-in allowed' : 'Sign-in turned off; their Macs keep their data')} label="Can sign in" />} />
              <Row
                title="Most Macs"
                trailing={<Picker label="Most Macs" value={String(user.max_instances)} options={['0', '1', '2', '3', '4', '6', '8', '10'].concat(['0', '1', '2', '3', '4', '6', '8', '10'].includes(String(user.max_instances)) ? [] : [String(user.max_instances)]).map((value) => ({ value, label: value === '0' ? 'None' : value }))} onChange={(value) => void patch({ max_instances: Number(value) }, 'Limit saved')} />}
              />
            </Section>
            <Section footer="A reset link lets them choose a new password once. It also turns off their two-factor authentication. To keep their encrypted Macs, they’ll need their recovery key.">
              {resetUrl ? (
                <CellBody>
                  <CopyField value={resetUrl} what="reset link" />
                </CellBody>
              ) : (
                <Row title="Create Password Reset Link…" tone="tint" onClick={() => setConfirmReset(true)} chevron={false} />
              )}
            </Section>
            {self ? null : (
              <Section footer={`Deletes ${user.display_name}’s account and every one of their Macs.`}>
                <Row title="Delete Person…" tone="destructive" onClick={() => setDeleting(true)} chevron={false} />
              </Section>
            )}
          </div>
        ) : null}
      </Sheet>
      <Confirm
        open={confirmReset}
        onOpenChange={setConfirmReset}
        title={`Create a reset link for ${user?.display_name ?? 'them'}?`}
        message="Anyone with the link can choose a new password for them, and it turns off their two-factor. They’ll need their recovery key to keep their encrypted Macs. Send it only to them."
        action="Create Link"
        onConfirm={() => void reset()}
      />
      <Sheet
        open={!!user && deleting}
        onOpenChange={(open) => {
          if (!open) {
            setDeleting(false);
            setTyped('');
          }
        }}
        title={`Delete ${user?.display_name ?? ''}?`}
        description={`Their account and ${count(user?.instance_count ?? 0, 'Mac')} are erased. This can’t be undone.`}
        primary={{ label: 'Delete', form: 'delete-user', disabled: typed.trim() !== user?.username, tone: 'destructive' }}
      >
        <form id="delete-user" className={styles.sheet} onSubmit={remove} noValidate>
          <Fields>
            <TextField label={`Type ${user?.username ?? ''} to confirm`} value={typed} onValueChange={setTyped} autoCapitalize="none" spellCheck={false} autoComplete="off" />
          </Fields>
        </form>
      </Sheet>
    </>
  );
}

const EXPIRES = [
  { value: '24', label: '1 day' },
  { value: '72', label: '3 days' },
  { value: '168', label: '1 week' },
] as const;

function Invites() {
  const invites = useQuery<Invite[]>(keys.invites, api.invites);
  const [creating, setCreating] = useState(false);
  const [role, setRole] = useState<Role>('member');
  const [hours, setHours] = useState<(typeof EXPIRES)[number]['value']>('72');
  const [created, setCreated] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revoking, setRevoking] = useState<Invite | null>(null);
  const now = Date.now();

  async function create() {
    setBusy(true);
    try {
      const result = await api.createInvite({ role, expires_in_hours: Number(hours) });
      updateData<Invite[]>(keys.invites, (list) => [result.invite, ...list]);
      setCreated(result.url);
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(false);
    }
  }
  async function revoke(invite: Invite) {
    try {
      await api.revokeInvite(invite.id);
      updateData<Invite[]>(keys.invites, (list) => list.filter((item) => item.id !== invite.id));
      notify.success('Invite revoked');
    } catch (error) {
      notify.error(error);
    }
  }

  const close = (open: boolean) => {
    setCreating(open);
    if (!open) setTimeout(() => setCreated(null), 400);
  };

  return (
    <Page title="Invites" back={back} actions={<Button variant="filled" onClick={() => setCreating(true)}>Create Invite</Button>}>
      {invites.error && !invites.data ? (
        <EmptyState icon="wifi" title="Couldn’t load invites">{invites.error.message}</EmptyState>
      ) : !invites.data ? (
        <div className={styles.gap}><SkeletonRows rows={2} /></div>
      ) : invites.data.length === 0 ? (
        <EmptyState icon="envelope" title="No invites" action={<Button variant="tinted" onClick={() => setCreating(true)}>Create Invite</Button>}>
          An invite link lets one person create an account and get their own Mac.
        </EmptyState>
      ) : (
        <Section footer="Each link works once.">
          {invites.data.map((invite) => {
            const expired = new Date(invite.expires_at).getTime() < now;
            return (
              <Row
                key={invite.id}
                title={invite.role === 'admin' ? 'Admin invite' : 'Member invite'}
                subtitle={`Created ${whenInline(invite.created_at, now)}${invite.used ? '' : ` · ${expired ? 'Expired' : `Expires ${day(invite.expires_at)}`}`}`}
                trailing={
                  invite.used ? (
                    <Pill tone="green">Used</Pill>
                  ) : expired ? (
                    <Pill tone="gray">Expired</Pill>
                  ) : (
                    <Button variant="plain" size="small" onClick={() => setRevoking(invite)}>
                      Revoke
                    </Button>
                  )
                }
              />
            );
          })}
        </Section>
      )}
      <Sheet open={creating} onOpenChange={close} title={created ? 'Invite Ready' : 'New Invite'} primary={created ? undefined : { label: 'Create Link', onClick: () => void create(), busy }} dismissLabel={created ? 'Done' : 'Cancel'}>
        <div className={styles.sheet}>
          {created ? (
            <>
              <p className="t-body pretty">Send this link to the person you’re inviting. It works once.</p>
              <CopyField value={created} what="invite link" />
            </>
          ) : (
            <>
              <div className={styles.field}>
                <span className="t-footnote secondary">They join as</span>
                <Segmented label="They join as" value={role} onChange={setRole} options={[{ value: 'member', label: 'Member' }, { value: 'admin', label: 'Admin' }]} />
              </div>
              <div className={styles.field}>
                <span className="t-footnote secondary">Link expires after</span>
                <Segmented label="Link expires after" value={hours} onChange={setHours} options={EXPIRES} />
              </div>
            </>
          )}
        </div>
      </Sheet>
      <Confirm open={!!revoking} onOpenChange={(open) => !open && setRevoking(null)} title="Revoke this invite?" message="The link stops working. You can create a new one any time." action="Revoke" onConfirm={() => revoking && withUndo('Revoking invite…', () => revoke(revoking))} />
    </Page>
  );
}

function AllMacs() {
  const instances = useQuery<AdminInstance[]>(keys.adminInstances, api.adminInstances, { pollMs: 15_000 });
  const [stopping, setStopping] = useState<AdminInstance | null>(null);
  async function stop(instance: AdminInstance) {
    try {
      await api.adminStop(instance.id);
      updateData<AdminInstance[]>(keys.adminInstances, (list) => list.map((item) => (item.id === instance.id ? { ...item, state: 'stopping' } : item)));
      notify.success(`Shutting down ${instance.name}`);
    } catch (error) {
      notify.error(error);
    }
  }
  return (
    <Page title="All Macs" back={back}>
      {instances.error && !instances.data ? (
        <EmptyState icon="wifi" title="Couldn’t load Macs">{instances.error.message}</EmptyState>
      ) : !instances.data ? (
        <div className={styles.gap}><SkeletonRows rows={4} /></div>
      ) : instances.data.length === 0 ? (
        <EmptyState icon="display" title="No Macs on this server yet" />
      ) : (
        <Section footer="Shutting down frees memory. Files stay.">
          {instances.data.map((instance) => {
            const info = stateInfo(instance.state);
            return (
              <Row
                key={instance.id}
                title={instance.name}
                subtitle={
                  <>
                    {instance.owner} · <StatusLabel tone={info.tone}>{info.label}</StatusLabel> · {instance.cpus} cores, {memory(instance.memory_mb)}
                    {instance.last_active_at ? ` · Used ${whenInline(instance.last_active_at)}` : ''}
                  </>
                }
                trailing={
                  isOn(instance.state) ? (
                    <Button variant="plain" size="small" onClick={() => setStopping(instance)}>
                      Shut Down
                    </Button>
                  ) : undefined
                }
              />
            );
          })}
        </Section>
      )}
      <Confirm open={!!stopping} onOpenChange={(open) => !open && setStopping(null)} title={`Shut down ${stopping?.name ?? ''}?`} message={`${stopping?.owner ?? 'Its owner'} loses anything unsaved on it, and their assistant’s current task stops.`} action="Shut Down" onConfirm={() => stopping && withUndo(`Shutting down ${stopping.name}…`, () => stop(stopping))} />
    </Page>
  );
}

const PHASE: Record<NonNullable<ImageStatus['builder']>['phase'], string> = {
  downloading: 'Downloading macOS',
  installing: 'Installing macOS',
  ready_to_promote: 'Ready to use',
  promoting: 'Saving as the base image',
  failed: 'Build failed',
};

function Image() {
  const { data: session } = useSession();
  const image = useQuery<ImageStatus>(keys.images, api.images, { pollMs: 30_000 });
  const builder = image.data?.builder ?? null;
  useLive(builder?.instance_id ?? null);
  const [version, setVersion] = useState<string | null>(null);
  const [busy, setBusy] = useState<'build' | 'promote' | 'discard' | null>(null);
  const [confirm, setConfirm] = useState<'promote' | 'discard' | null>(null);

  async function act(kind: 'build' | 'promote' | 'discard') {
    setBusy(kind);
    try {
      const next = kind === 'build' ? await api.startBuilder(version ?? image.data!.versions[0]!.id) : kind === 'promote' ? await api.promoteImage() : await api.discardBuilder();
      setData(keys.images, next);
      notify.success(kind === 'build' ? 'Building a new image' : kind === 'promote' ? 'New Macs will start from this image' : 'Build discarded');
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  const data = image.data;
  return (
    <Page title="macOS Image" back={back}>
      {image.error && !data ? (
        <EmptyState icon="wifi" title="Couldn’t load the image status">{image.error.message}</EmptyState>
      ) : !data ? (
        <div className={styles.gap}><SkeletonRows rows={3} /></div>
      ) : (
        <>
          {!data.base.present && !builder ? (
            <div className={styles.gap}>
              <Banner tone="orange" icon="disc" title="No base image yet">
                People can’t create Macs until there is one. Build it below; it takes about an hour, most of it unattended.
              </Banner>
            </div>
          ) : null}
          <Section title="Current image" footer="New Macs start as a copy of this image.">
            <Row title="macOS" value={data.base.macos ?? 'None'} />
            {data.base.present ? <Row title="Size" value={bytes(data.base.size_bytes)} /> : null}
            {data.base.created_at ? <Row title="Built" value={day(data.base.created_at)} /> : null}
            <Row title="Firmware" subtitle={data.firmware.detail} trailing={<StatusLabel tone={data.firmware.present ? 'green' : 'red'}>{data.firmware.present ? 'Installed' : 'Missing'}</StatusLabel>} align="top" />
          </Section>

          <Section title={builder ? 'New image' : 'Build a new image'}>
            {builder ? (
              <>
                <CellBody>
                  <div className={styles.build}>
                    <div className={styles.buildHead}>
                      <span className="t-headline">{PHASE[builder.phase]}</span>
                      <span className="t-subhead secondary tabular">{builder.progress !== null && builder.phase !== 'ready_to_promote' ? `${Math.round(builder.progress * 100)}%` : ''}</span>
                    </div>
                    {builder.phase === 'failed' || builder.phase === 'ready_to_promote' ? null : <ProgressBar value={builder.progress} label={PHASE[builder.phase]} />}
                    <p className={`t-subhead ${builder.phase === 'failed' ? 'text-red' : 'secondary'} pretty`}>{builder.detail}</p>
                  </div>
                </CellBody>
                {builder.phase === 'installing' ? <Row title="Open the Installer’s Screen" href="/admin/image/screen" leading={<GlyphTile color="indigo"><Icon name="display" size={17} weight={2} /></GlyphTile>} /> : null}
                {builder.phase === 'ready_to_promote' ? (
                  <CellBody>
                    <Button variant="filled" onClick={() => setConfirm('promote')} busy={busy === 'promote'}>
                      Use as Base Image
                    </Button>
                  </CellBody>
                ) : null}
                {builder.phase !== 'promoting' ? <Row title={builder.phase === 'failed' ? 'Discard and Start Over' : 'Discard Build…'} tone="destructive" onClick={() => setConfirm('discard')} chevron={false} /> : null}
              </>
            ) : (
              <>
                <Row title="Version" trailing={<Picker label="macOS version" value={version ?? data.versions[0]?.id ?? ''} options={data.versions.map((item) => ({ value: item.id, label: item.label }))} onChange={setVersion} />} />
                <CellBody>
                  <div className={styles.buildAction}>
                    <p className="t-subhead secondary pretty">Downloads macOS from Apple. You finish setup on its screen.</p>
                    <Button variant="filled" onClick={() => void act('build')} busy={busy === 'build'} disabled={!data.versions.length || !data.firmware.present}>
                      Build Image
                    </Button>
                  </div>
                </CellBody>
              </>
            )}
          </Section>
          {builder && (builder.phase === 'installing' || builder.phase === 'ready_to_promote') ? <BuilderSteps vmUrl={session?.vm_url ?? location.origin} /> : null}
        </>
      )}
      <Confirm open={confirm === 'promote'} onOpenChange={(open) => !open && setConfirm(null)} tone="default" title="Use this as the base image?" message="New Macs start from it. Existing Macs keep running on the image they started with. The temporary Mac shuts down." action="Use as Base Image" onConfirm={() => void act('promote')} />
      <Confirm open={confirm === 'discard'} onOpenChange={(open) => !open && setConfirm(null)} title="Discard this build?" message="The temporary Mac and its download are deleted." action="Discard" onConfirm={() => withUndo('Discarding build…', () => act('discard'))} />
    </Page>
  );
}

const RUNNING = ['1', '2', '3', '4', '6', '8', '12', '16'];

function Policies() {
  const overview = useQuery<HostOverview & { require_totp?: boolean }>(keys.overview, api.overview);
  const { data: session } = useSession();
  const [budget, setBudget] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const data = overview.data;

  async function save(body: Parameters<typeof api.settings>[0], message: string) {
    try {
      setData(keys.overview, await api.settings(body));
      if (body.require_totp !== undefined && session) setData(keys.session, { ...session, require_totp: body.require_totp });
      notify.success(message);
    } catch (cause) {
      notify.error(cause);
    }
  }
  async function saveBudget(event: FormEvent) {
    event.preventDefault();
    const gb = Number(budget);
    if (!Number.isFinite(gb) || gb < 4) {
      setError('Use at least 4 GB.');
      return;
    }
    if (data && gb * 1024 > data.memory_total_mb) {
      setError(`This server has ${memory(data.memory_total_mb)}. Leave some for the server itself.`);
      return;
    }
    setError(null);
    await save({ memory_budget_mb: Math.round(gb * 1024) }, 'Memory budget saved');
    setBudget(null);
  }

  return (
    <Page title="Policies" back={back}>
      {!data ? (
        overview.error ? <EmptyState icon="wifi" title="Couldn’t load settings">{overview.error.message}</EmptyState> : <div className={styles.gap}><SkeletonRows rows={3} /></div>
      ) : (
        <>
          <Section footer="People without it set it up before using their Macs.">
            <Row title="Require two-factor authentication" trailing={<Switch checked={data.require_totp ?? !!session?.require_totp} onCheckedChange={(on) => void save({ require_totp: on }, on ? 'Two-factor now required' : 'Two-factor now optional')} label="Require two-factor authentication" />} />
          </Section>
          <Section title="Running Macs" footer={`This server has ${memory(data.memory_total_mb)} of memory and ${data.cpu_count} cores.`}>
            <CellBody>
              <form className={styles.budget} onSubmit={saveBudget}>
                <div className={styles.budgetField}>
                  <TextField label="Memory for Macs (GB)" value={budget ?? String(Math.round(data.budget.memory_mb / 1024))} onValueChange={setBudget} inputMode="numeric" error={error} />
                </div>
                <Button type="submit" variant="tinted" disabled={budget === null}>
                  Save
                </Button>
              </form>
            </CellBody>
            <Row title="Most Macs running at once" trailing={<Picker label="Most Macs running at once" value={String(data.budget.max_running)} options={(RUNNING.includes(String(data.budget.max_running)) ? RUNNING : [...RUNNING, String(data.budget.max_running)]).map((value) => ({ value, label: value }))} onChange={(value) => void save({ max_running: Number(value) }, 'Limit saved')} />} />
          </Section>
        </>
      )}
    </Page>
  );
}


/** What the admin does on the installer's screen, in order. */
function BuilderSteps({ vmUrl }: { vmUrl: string }) {
  // The installer's Mac reaches Familiar at its internal address; the public one may not loop back.
  const base = vmUrl.replace(/\/+$/, '');
  return (
    <Section title="Finish installing macOS" footer="The doctor checks permissions, automatic login and the agent. When it reports no problems, come back and choose Use as Base Image.">
      <CellBody>
        <ol className={styles.steps}>
          <li>
            Open the <a href="/admin/image/screen" onClick={(event) => { event.preventDefault(); navigate('/admin/image/screen'); }}>installer’s screen</a> and choose Take Control.
          </li>
          <li>In the boot picker, use the arrow keys to select macOS Base System, then press Return. The pointer works once the installer opens.</li>
          <li>Open Disk Utility, erase the largest disk as APFS, then quit Disk Utility.</li>
          <li>Choose Reinstall macOS and follow the installer. It restarts a few times.</li>
          <li>Finish setup, then turn on automatic login for that user in System Settings &gt; Users &amp; Groups.</li>
          <li>
            In Terminal, run:
            <div className={styles.commands}>
              <CopyField value={`curl -fsSL ${base}/guest/install.sh | sh`} what="install command" prefix="$" />
              <CopyField value="dot-guest prepare-image" what="prepare command" prefix="$" />
              <CopyField value="dot-guest doctor" what="doctor command" prefix="$" />
            </div>
          </li>
        </ol>
      </CellBody>
    </Section>
  );
}
