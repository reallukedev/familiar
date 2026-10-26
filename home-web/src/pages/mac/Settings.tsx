import { useEffect, useState, type FormEvent } from 'react';
import { Button } from '../../components/Button';
import { Choice } from '../../components/Choice';
import { CopyField } from '../../components/Copy';
import { Spinner } from '../../components/Spinner';
import { LimitInput } from '../../components/storage/LimitInput';
import { Switch } from '../../components/Switch';
import { Fields, FormError, TextField } from '../../components/Field';
import { Icon } from '../../components/Icon';
import { ProgressBar } from '../../components/Meter';
import { CellBody, GlyphTile, Row, Section } from '../../components/List';
import { Picker } from '../../components/Picker';
import { Segmented } from '../../components/Segmented';
import { Sheet } from '../../components/Sheet';
import { ApiError, api } from '../../lib/api';
import { memory, minutes } from '../../lib/format';
import { putInstance, removeInstance } from '../../lib/live';
import { navigate, scrollToHash } from '../../lib/router';
import { macPath } from '../../lib/routes';
import { useSession } from '../../lib/session';
import { isOn } from '../../lib/status';
import { notify } from '../../lib/toast';
import { withUnlock } from '../../lib/unlock';
import type { AgentChannel, Instance, InstanceAgent } from '../../lib/types';
import { agentChannelLabel, CHANNEL_DETAIL } from '../../lib/updates';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Settings.module.css';
import { withUndo } from '../../lib/undo';

const SLEEP = ['0', '15', '30', '60', '120', '240'] as const;
const HIBERNATE = ['0', '60', '240', '720', '1440'] as const;
const CPUS = ['2', '4', '6', '8'] as const;
const MEMORY = ['4096', '8192', '12288', '16384'] as const;

function withCurrent<T extends string>(values: readonly T[], current: number): string[] {
  const list: string[] = [...values];
  if (!list.includes(String(current))) list.push(String(current));
  return list.sort((a, b) => Number(a) - Number(b));
}

export function SettingsPage({ instance }: MacPageProps) {
  const [name, setName] = useState(instance.name);
  const [savingName, setSavingName] = useState(false);
  const [deleting, setDeleting] = useState(false);
  useEffect(() => setName(instance.name), [instance.name]);
  // The Mac's page links straight to Agent updates.
  useEffect(() => {
    scrollToHash('start');
  }, []);

  async function update(patch: Parameters<typeof api.updateInstance>[1], message?: string) {
    try {
      putInstance(await api.updateInstance(instance.id, patch));
      if (message) notify.success(message);
    } catch (error) {
      notify.error(error);
    }
  }

  async function saveName(event: FormEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed || trimmed === instance.name) return;
    setSavingName(true);
    try {
      await update({ name: trimmed }, 'Name saved');
    } finally {
      setSavingName(false);
    }
  }

  const running = isOn(instance.state);
  const vm = instance.driver === 'qemu';

  return (
    <Page title="Settings" back={{ href: macPath(instance.id), label: instance.name }}>
      <Section title="Name">
        <CellBody>
          <form className={styles.nameForm} onSubmit={saveName}>
            <div className={styles.nameField}>
              <TextField label="Name" labelHidden value={name} onValueChange={setName} maxLength={64} />
            </div>
            <Button type="submit" variant="tinted" disabled={!name.trim() || name.trim() === instance.name} busy={savingName}>
              Save
            </Button>
          </form>
        </CellBody>
      </Section>

      {vm ? (
        <Section title="Resources" footer={running ? 'Changes apply the next time the Mac starts.' : 'Changes apply when the Mac starts.'}>
          <CellBody>
            <div className={styles.resource}>
              <span className="t-body">Processor cores</span>
              <Segmented label="Processor cores" value={String(instance.cpus)} onChange={(value) => void update({ cpus: Number(value) }, `${value} cores${running ? ' after the next start' : ''}`)} options={withCurrent(CPUS, instance.cpus).map((value) => ({ value, label: value }))} />
            </div>
          </CellBody>
          <CellBody>
            <div className={styles.resource}>
              <span className="t-body">Memory</span>
              <Segmented label="Memory" value={String(instance.memory_mb)} onChange={(value) => void update({ memory_mb: Number(value) }, `${memory(Number(value))}${running ? ' after the next start' : ''}`)} options={withCurrent(MEMORY, instance.memory_mb).map((value) => ({ value, label: memory(Number(value)) }))} />
            </div>
          </CellBody>
          <Row title="Disk" value={`${instance.disk_gb} GB`} />
        </Section>
      ) : null}

      {vm ? (
        <Section title="Energy">
          <Row
            title="Sleep when idle for"
            trailing={<Picker label="Sleep when idle for" value={String(instance.sleep_after_minutes)} options={withCurrent(SLEEP, instance.sleep_after_minutes).map((value) => ({ value, label: minutes(Number(value)) }))} onChange={(value) => void update({ sleep_after_minutes: Number(value) })} />}
          />
          <Row
            title="Hibernate when idle for"
            trailing={<Picker label="Hibernate when idle for" value={String(instance.hibernate_after_minutes)} options={withCurrent(HIBERNATE, instance.hibernate_after_minutes).map((value) => ({ value, label: minutes(Number(value)) }))} onChange={(value) => void update({ hibernate_after_minutes: Number(value) })} />}
          />
        </Section>
      ) : (
        <Section title="Connection">
          <Row title="Address" value={instance.remote_host ?? ''} />
        </Section>
      )}

      {vm ? <SnapshotSettings instance={instance} update={update} /> : null}

      {vm ? <EncryptionSettings instance={instance} /> : null}

      {instance.agent ? <AgentUpdates instance={instance} agent={instance.agent} /> : null}

      <div className={styles.danger}>
        <Section footer={'This can’t be undone.'}>
          <Row title="Delete Mac…" tone="destructive" onClick={() => setDeleting(true)} chevron={false} />
        </Section>
      </div>
      <DeleteMacSheet instance={instance} open={deleting} onOpenChange={setDeleting} />
    </Page>
  );
}

type SnapshotPatch = { snapshot_keep_days?: number; snapshot_max_gb?: number };

/** A whole number from min to max typed into a field, or null. */
function whole(text: string, min: number, max: number): number | null {
  const clean = text.replace(/[,\s]/g, '');
  if (!/^\d+$/.test(clean)) return null;
  const value = Number(clean);
  return value >= min && value <= max ? value : null;
}

/** One number setting with an off switch: "Keep snapshots for [7] days" + "Forever". */
function useNumberSetting(saved: number, fallback: number, min: number, max: number, save: (value: number) => Promise<void>) {
  const [text, setText] = useState(saved > 0 ? String(saved) : String(fallback));
  const [error, setError] = useState(false);
  useEffect(() => {
    if (saved > 0) setText(String(saved));
  }, [saved]);
  return {
    text,
    error,
    edit(value: string) {
      setText(value);
      setError(false);
    },
    commit() {
      const value = whole(text, min, max);
      if (value === null) return setError(true);
      setText(String(value));
      if (value !== saved) void save(value);
    },
    toggle(off: boolean) {
      setError(false);
      void save(off ? 0 : (whole(text, min, max) ?? fallback));
    },
  };
}

/** How long snapshots stay and how much space they may take before old ones are deleted. */
function SnapshotSettings({ instance, update }: { instance: Instance; update: (patch: SnapshotPatch) => Promise<void> }) {
  const keepDays = instance.snapshot_keep_days ?? 7;
  const maxGb = instance.snapshot_max_gb ?? 0;
  const days = useNumberSetting(keepDays, 7, 1, 3650, (value) => update({ snapshot_keep_days: value }));
  const space = useNumberSetting(maxGb, 50, 1, 100_000, (value) => update({ snapshot_max_gb: value }));
  const forever = keepDays <= 0;
  const unlimited = maxGb <= 0;
  const field = (id: string, setting: ReturnType<typeof useNumberSetting>, unit: string, disabled: boolean) => (
    <LimitInput
      id={id}
      aria-labelledby={`${id}-label`}
      value={setting.text}
      onValueChange={setting.edit}
      onBlur={setting.commit}
      onKeyDown={(event) => {
        if (event.key === 'Enter') setting.commit();
      }}
      unit={unit}
      invalid={setting.error}
      disabled={disabled}
    />
  );
  return (
    <Section title="Snapshots" footer="Older snapshots are deleted automatically. Ones you mark Keep stay.">
      <Row
        title={<span id="snapshot-days-label">Keep snapshots for</span>}
        subtitle={days.error ? <span className="text-red">Use 1 to 3,650 days.</span> : undefined}
        trailing={field('snapshot-days', days, 'days', forever)}
      />
      <Row title="Forever" trailing={<Switch label="Keep snapshots forever" checked={forever} onCheckedChange={days.toggle} />} />
      <Row
        title={<span id="snapshot-space-label">Most space for snapshots</span>}
        subtitle={space.error ? <span className="text-red">Use 1 to 100,000 GB.</span> : undefined}
        trailing={field('snapshot-space', space, 'GB', unlimited)}
      />
      <Row title="No limit" trailing={<Switch label="No limit on snapshot space" checked={unlimited} onCheckedChange={space.toggle} />} />
    </Section>
  );
}

/** Whether this Mac's data on the server is encrypted, and the way to encrypt an older one. */
function EncryptionSettings({ instance }: { instance: Instance }) {
  const [asking, setAsking] = useState(false);
  if (instance.state === 'encrypting') {
    const value = typeof instance.encrypt_progress === 'number' ? instance.encrypt_progress : null;
    return (
      <Section title="Encryption" footer={`${instance.name} can start when this finishes.`} live="polite">
        <CellBody>
          <div className={styles.encrypting}>
            <div className={styles.encryptingLine}>
              <span className="t-body">Encrypting…</span>
              <span className="t-subhead secondary tabular">{value === null ? '' : `${Math.round(value * 100)}%`}</span>
            </div>
            <ProgressBar value={value} label={`Encrypting ${instance.name}`} />
          </div>
        </CellBody>
      </Section>
    );
  }
  if (instance.encrypted) {
    return (
      <Section title="Encryption">
        <Row
          align="top"
          leading={
            <GlyphTile color="green">
              <Icon name="lock" size={16} weight={2.1} />
            </GlyphTile>
          }
          title="Encrypted"
          subtitle="Its disk, snapshots and saved memory are encrypted with your password."
        />
      </Section>
    );
  }
  if (!instance.can_encrypt) return null;
  return (
    <Section title="Encryption" footer="Macs made now are encrypted from the start.">
      <Row
        align="top"
        leading={
          <GlyphTile color="orange">
            <Icon name="lock" size={16} weight={2.1} />
          </GlyphTile>
        }
        title="Not encrypted"
        subtitle="This Mac was made before encryption, so its disk is stored as is on the server."
        trailing={
          <Button variant="tinted" size="small" onClick={() => setAsking(true)}>
            Encrypt This Mac…
          </Button>
        }
      />
      <EncryptSheet instance={instance} open={asking} onOpenChange={setAsking} />
    </Section>
  );
}

function EncryptSheet({ instance, open, onOpenChange }: { instance: Instance; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const off = instance.state === 'stopped' || instance.state === 'error';

  async function encrypt() {
    setBusy(true);
    setError(null);
    try {
      putInstance(await withUnlock(() => api.encryptInstance(instance.id)));
      onOpenChange(false);
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.silent)) setError(cause instanceof ApiError ? cause.message : 'Couldn’t start encrypting. Try again.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        if (!next) setError(null);
        onOpenChange(next);
      }}
      title={`Encrypt ${instance.name}?`}
      description="Its disk, snapshots and saved memory will be encrypted with a key only your password or recovery key can unlock."
      primary={{ label: 'Encrypt', onClick: () => void encrypt(), disabled: !off, busy }}
    >
      <div className={styles.encryptBody}>
        <ul className={`t-subhead ${styles.encryptList}`}>
          <li>It takes a few minutes, and the Mac stays off until it’s done.</li>
          <li>Its snapshots are removed.</li>
          <li>If you forget your password and lose your recovery key, its data can’t be recovered.</li>
        </ul>
        {off ? null : <p className="t-subhead text-orange">Shut down {instance.name} first.</p>}
        <FormError>{error}</FormError>
      </div>
    </Sheet>
  );
}

const AGENT_CHANNELS: AgentChannel[] = ['default', 'stable', 'nightly', 'off'];

/** The Familiar agent inside the Mac: its version, the channel it follows, and updating it now. */
function AgentUpdates({ instance, agent }: { instance: Instance; agent: InstanceAgent }) {
  const { data: session } = useSession();
  const [busy, setBusy] = useState(false);
  const running = instance.state === 'ready' || instance.state === 'degraded';
  const base = (session?.public_url ?? location.origin).replace(/\/+$/, '');

  async function setChannel(channel: AgentChannel) {
    try {
      putInstance(await api.setAgentChannel(instance.id, channel));
    } catch (error) {
      notify.error(error);
    }
  }
  async function updateNow() {
    setBusy(true);
    try {
      putInstance(await api.updateAgent(instance.id));
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(false);
    }
  }

  const options = AGENT_CHANNELS.map((value) => ({
    value,
    label: agentChannelLabel(value, agent),
    detail: value === 'default' ? 'Follows what the server’s admin chooses.' : value === 'off' ? 'Stays on the version it has.' : CHANNEL_DETAIL[value],
  }));
  return (
    <Section title="Agent updates" id="agent">
      <Row title="Familiar agent" value={agent.version ?? 'Not connected yet'} />
      <Row title="Channel" trailing={<Choice label="Agent channel" value={agent.channel} options={options} onChange={(value) => void setChannel(value)} disabled={agent.updating} />} />
      {agent.updating ? (
        <Row title={`Updating to ${agent.available ?? 'the new version'}…`} trailing={<Spinner size={18} label="Updating" />} />
      ) : agent.can_update && agent.available ? (
        <Row
          title={`Agent ${agent.available} is available`}
          subtitle={running ? undefined : `Start ${instance.name} to update it.`}
          trailing={
            <Button variant="tinted" size="small" onClick={() => void updateNow()} busy={busy} disabled={!running}>
              Update Now
            </Button>
          }
        />
      ) : null}
      {agent.can_update ? null : (
        <CellBody>
          <div className={styles.reinstall}>
            <p className="t-subhead secondary pretty">This Mac’s agent is too old to update itself. To reinstall it, open Terminal on the Mac and run this command.</p>
            <CopyField value={`curl -fsSL ${base}/guest/install.sh | sh`} what="reinstall command" prefix="$" />
          </div>
        </CellBody>
      )}
    </Section>
  );
}

function DeleteMacSheet({ instance, open, onOpenChange }: { instance: Instance; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [typed, setTyped] = useState('');
  const matches = typed.trim() === instance.name;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!matches) return;
    onOpenChange(false);
    withUndo(`Deleting ${instance.name}…`, async () => {
      try {
        await api.deleteInstance(instance.id, instance.name);
        removeInstance(instance.id);
        notify.success(`Deleted ${instance.name}`);
        navigate('/', { replace: true, direction: 'back' });
      } catch (cause) {
        notify.error(cause);
      }
    });
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange} title={`Delete ${instance.name}?`} primary={{ label: 'Delete Mac', form: 'delete-mac', disabled: !matches, tone: 'destructive' }} description="Everything on this Mac is erased: files, apps, snapshots, activity and the tokens that reach it. Assistants connected to it stop working.">
      <form id="delete-mac" className={styles.deleteForm} onSubmit={submit} noValidate>
        <Fields>
          <TextField label={`Type ${instance.name} to confirm`} value={typed} onValueChange={setTyped} autoComplete="off" autoCapitalize="none" spellCheck={false} />
        </Fields>
      </form>
    </Sheet>
  );
}
