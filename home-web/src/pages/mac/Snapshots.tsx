import { useState, type FormEvent } from 'react';
import { EmptyState, Pill } from '../../components/Bits';
import { Button, IconButton } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { Fields, FormError, TextField } from '../../components/Field';
import { Icon } from '../../components/Icon';
import { Row, Section } from '../../components/List';
import { ActionMenu } from '../../components/Menu';
import { Sheet } from '../../components/Sheet';
import { SkeletonRows } from '../../components/Skeleton';
import { ApiError, api } from '../../lib/api';
import { withUnlock } from '../../lib/unlock';
import { invalidate, keys, updateData, useQuery } from '../../lib/cache';
import { bytes, stamp, when, whenInline } from '../../lib/format';
import { putInstance } from '../../lib/live';
import { usePlatform } from '../../lib/platform';
import { macPath } from '../../lib/routes';
import { isOn } from '../../lib/status';
import { notify } from '../../lib/toast';
import type { Snapshot, SnapshotUsage } from '../../lib/types';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Snapshots.module.css';
import { withUndo } from '../../lib/undo';

const HOUR = 3_600_000;

/** "Deletes in 5 days", "Deletes in 3 hours", "Deletes soon". */
function deletesIn(iso: string, now: number): string {
  const hours = Math.floor((Date.parse(iso) - now) / HOUR);
  if (hours < 1) return 'Deletes soon';
  if (hours < 24) return `Deletes in ${hours} ${hours === 1 ? 'hour' : 'hours'}`;
  const days = Math.round(hours / 24);
  return `Deletes in ${days} ${days === 1 ? 'day' : 'days'}`;
}

/** "12 GB of 50 GB", with a note when kept snapshots alone are over the cap. */
function usageFooter(usage: SnapshotUsage | undefined): string {
  if (!usage?.limit_bytes) return '';
  if (usage.kept_over_limit) return 'Kept snapshots use more than the limit.';
  return `${usage.used_bytes > 0 ? bytes(usage.used_bytes) : '0 GB'} of ${bytes(usage.limit_bytes)} used.`;
}

export function SnapshotsPage({ instance }: MacPageProps) {
  const phone = usePlatform() === 'phone';
  // Always refetched on arrival: the sweeper may have deleted some since.
  const snapshots = useQuery<Snapshot[]>(keys.snapshots(instance.id), () => api.snapshots(instance.id), { staleMs: 0 });
  const usage = useQuery<SnapshotUsage>(instance.driver === 'remote' ? null : `snapshot-usage:${instance.id}`, () => api.snapshotUsage(instance.id), { staleMs: 0 });
  const [creating, setCreating] = useState(false);
  const [restoring, setRestoring] = useState<Snapshot | null>(null);
  const [deleting, setDeleting] = useState<Snapshot | null>(null);
  const now = Date.now();
  const remote = instance.driver === 'remote';

  async function restore(snapshot: Snapshot) {
    try {
      putInstance(await withUnlock(() => api.restoreSnapshot(instance.id, snapshot.id)));
      notify.success(`Restoring “${snapshot.name}”`);
    } catch (error) {
      notify.error(error);
    }
  }
  async function remove(snapshot: Snapshot) {
    try {
      await withUnlock(() => api.deleteSnapshot(instance.id, snapshot.id));
      updateData<Snapshot[]>(keys.snapshots(instance.id), (list) => list.filter((item) => item.id !== snapshot.id));
      notify.success(`Deleted “${snapshot.name}”`);
      void usage.reload();
    } catch (error) {
      notify.error(error);
    }
  }
  async function keep(snapshot: Snapshot, kept: boolean) {
    try {
      const next = await api.keepSnapshot(instance.id, snapshot.id, kept);
      updateData<Snapshot[]>(keys.snapshots(instance.id), (list) => list.map((item) => (item.id === next.id ? next : item)));
      void usage.reload();
    } catch (error) {
      notify.error(error);
    }
  }

  const live = isOn(instance.state);
  return (
    <Page
      title="Snapshots"
      back={{ href: macPath(instance.id), label: instance.name }}
      actions={
        remote ? undefined : phone ? (
          <IconButton label="Take snapshot" onClick={() => setCreating(true)}>
            <Icon name="plus" size={22} weight={2.2} />
          </IconButton>
        ) : (
          <Button variant="filled" onClick={() => setCreating(true)}>
            Take Snapshot
          </Button>
        )
      }
    >
      {remote ? (
        <EmptyState icon="stack" title="Snapshots are for Macs on this server">Use Time Machine on a Mac on your network.</EmptyState>
      ) : snapshots.error && !snapshots.data ? (
        <EmptyState icon="wifi" title="Couldn’t load snapshots" action={<Button variant="tinted" onClick={() => void snapshots.reload()}>Try Again</Button>}>
          {snapshots.error.message}
        </EmptyState>
      ) : !snapshots.data ? (
        <div className={styles.loading}>
          <SkeletonRows rows={3} label="Loading snapshots" />
        </div>
      ) : snapshots.data.length === 0 ? (
        <EmptyState icon="stack" title="No snapshots yet" action={<Button variant="tinted" onClick={() => setCreating(true)}>Take Snapshot</Button>}>
          A snapshot saves the whole Mac as it is now, so you can go back to it if something goes wrong. Your assistant can take one before risky work too.
        </EmptyState>
      ) : (
        <Section
          footer={[
            live ? 'Restoring returns the Mac to that exact moment.' : 'Restoring starts the Mac from that point.',
            usageFooter(usage.data),
          ]
            .filter(Boolean)
            .join(' ')}
        >
          {snapshots.data.map((snapshot) => (
            <Row
              key={snapshot.id}
              title={snapshot.name}
              subtitle={
                <span title={stamp(snapshot.created_at)}>
                  {when(snapshot.created_at, now)}
                  {snapshot.size_bytes ? ` · ${bytes(snapshot.size_bytes)}` : ''}
                  {snapshot.live ? ' · Includes memory' : ''}
                  {snapshot.expires_at ? (
                    <span className="t-footnote" style={{ display: 'block' }}>
                      {deletesIn(snapshot.expires_at, now)}
                    </span>
                  ) : null}
                </span>
              }
              trailing={
                <>
                  {snapshot.kept ? <Pill tone="blue">Kept</Pill> : null}
                  {snapshot.created_by === 'agent' ? <Pill tone="gray">By assistant</Pill> : null}
                  <ActionMenu
                    label={`Actions for ${snapshot.name}`}
                    trigger={
                      <IconButton label={`Actions for ${snapshot.name}`}>
                        <Icon name="ellipsis" size={22} weight={3} />
                      </IconButton>
                    }
                    items={[
                      { label: 'Restore…', onSelect: () => setRestoring(snapshot) },
                      snapshot.kept
                        ? { label: 'Don’t Keep', onSelect: () => void keep(snapshot, false) }
                        : { label: 'Keep', onSelect: () => void keep(snapshot, true) },
                      { label: 'Delete…', onSelect: () => setDeleting(snapshot), tone: 'destructive', separated: true },
                    ]}
                  />
                </>
              }
            />
          ))}
        </Section>
      )}

      <CreateSnapshotSheet instanceId={instance.id} open={creating} onOpenChange={setCreating} />
      <Confirm
        open={!!restoring}
        onOpenChange={(open) => !open && setRestoring(null)}
        title={`Restore “${restoring?.name ?? ''}”?`}
        message={`${instance.name} restarts and goes back to how it was ${restoring ? whenInline(restoring.created_at, now) : ''}. Anything done on it since then is lost, unless you take a snapshot first.`}
        action="Restore and Restart"
        onConfirm={() => restoring && withUndo(`Restoring “${restoring.name}”…`, () => restore(restoring))}
      />
      <Confirm
        open={!!deleting}
        onOpenChange={(open) => !open && setDeleting(null)}
        title={`Delete “${deleting?.name ?? ''}”?`}
        message="The Mac stays as it is now; you just can’t go back to this point any more."
        action="Delete Snapshot"
        onConfirm={() => deleting && withUndo(`Deleting “${deleting.name}”…`, () => remove(deleting))}
      />
    </Page>
  );
}

function CreateSnapshotSheet({ instanceId, open, onOpenChange }: { instanceId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit(event: FormEvent) {
    event.preventDefault();
    const value = name.trim() || `Snapshot ${new Date().toLocaleString('en', { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}`;
    setBusy(true);
    setError(null);
    try {
      const snapshot = await withUnlock(() => api.createSnapshot(instanceId, value));
      updateData<Snapshot[]>(keys.snapshots(instanceId), (list) => [snapshot, ...list]);
      // A cap may have deleted older snapshots to make room.
      invalidate(keys.snapshots(instanceId));
      invalidate(`snapshot-usage:${instanceId}`);
      notify.success('Snapshot saved');
      onOpenChange(false);
      setName('');
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : 'Couldn’t take the snapshot. Try again.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Take Snapshot" primary={{ label: 'Take Snapshot', form: 'new-snapshot', busy }}>
      <form id="new-snapshot" className={styles.form} onSubmit={submit} noValidate>
        <Fields>
          <TextField label="Name" value={name} onValueChange={setName} placeholder="Before installing Xcode…" maxLength={80} description="Leave it blank to name it after the date and time." />
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}
