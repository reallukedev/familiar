import { useState } from 'react';
import { Banner, EmptyState } from '../components/Bits';
import { Button } from '../components/Button';
import { Confirm } from '../components/Confirm';
import { Icon, type IconName } from '../components/Icon';
import { CellBody, GlyphTile, Row, Section } from '../components/List';
import { SkeletonBlock, SkeletonRows } from '../components/Skeleton';
import { CapacityBar, Legend, Swatch } from '../components/storage/CapacityBar';
import { api } from '../lib/api';
import { invalidate, keys, setData, updateData, useQuery } from '../lib/cache';
import { bytes, count } from '../lib/format';
import { usePlatform } from '../lib/platform';
import { navigate, onLinkClick, useSearchParam } from '../lib/router';
import { macPath } from '../lib/routes';
import { useSession } from '../lib/session';
import {
  GB, capacity, diskLabel, isLocal, kindSegments, macSegments, quotaLabel, snapshotsLabel, sortMacs, sourceLabel, type Win,
} from '../lib/storage';
import { notify } from '../lib/toast';
import type { Instance, MacStorage, MyStorage } from '../lib/types';
import { Page } from '../shells/Page';
import { Sheet } from '../components/Sheet';
import styles from './Storage.module.css';
import { withUndo } from '../lib/undo';

/** "My storage": what the person's Macs keep on the server, and how to free it. */
export function StoragePage() {
  const query = useQuery<MyStorage>(keys.storage, api.storage, { pollMs: 60_000 });
  const phone = usePlatform() === 'phone';
  const focused = useSearchParam('mac');
  // Kept after closing so the alert's text doesn't change while it animates out.
  const [discard, setDiscard] = useState<{ mac: MacStorage | null; open: boolean }>({ mac: null, open: false });
  const setDiscarding = (mac: MacStorage) => setDiscard({ mac, open: true });
  const data = query.data;

  const detail = phone && focused && data ? data.macs.find((mac) => mac.instance_id === focused) : null;

  const content = query.error && !data ? (
    <EmptyState icon="wifi" title="Couldn’t load your storage" action={<Button variant="tinted" onClick={() => void query.reload()}>Try Again</Button>}>
      {query.error.message}
    </EmptyState>
  ) : !data ? (
    <Loading />
  ) : detail ? (
    <MacDetail mac={detail} onDiscard={setDiscarding} />
  ) : (
    <Overview data={data} phone={phone} onDiscard={setDiscarding} />
  );

  return (
    <>
      {detail ? (
        <Page title={detail.name} back={{ href: '/storage', label: 'Storage' }}>
          {content}
        </Page>
      ) : (
        <Page title="Storage">{content}</Page>
      )}
      <DiscardMemory mac={discard.mac} open={discard.open} onClose={() => setDiscard((value) => ({ ...value, open: false }))} />
    </>
  );
}

function Loading() {
  return (
    <div className={styles.stack} role="status" aria-label="Loading storage">
      <Section>
        <CellBody>
          <SkeletonBlock height={92} radius={8} />
        </CellBody>
      </Section>
      <SkeletonRows rows={4} leading />
    </div>
  );
}

function Overview({ data, phone, onDiscard }: { data: MyStorage; phone: boolean; onDiscard: (mac: MacStorage) => void }) {
  const cap = capacity(data.used_bytes, data.limits.quota_gb);
  const macs = sortMacs(data.macs);
  const [info, setInfo] = useState<MacStorage | null>(null);
  const [limits, setLimits] = useState(false);
  const shown = info ? (data.macs.find((mac) => mac.instance_id === info.instance_id) ?? info) : null;

  return (
    <div className={styles.stack}>
      {data.over_quota ? (
        <div className={styles.banner}>
          <Banner tone="red" icon="warning" title={cap.over > 0 ? `${bytes(cap.over)} over your ${quotaLabel(data.limits.quota_gb)} limit` : `You’ve reached your ${quotaLabel(data.limits.quota_gb)} limit`}>
            You can’t create Macs or take snapshots until you free some space. Your Macs keep running, and nothing is deleted.
          </Banner>
        </div>
      ) : null}

      <Summary data={data} onLimits={phone ? undefined : () => setLimits(true)} />

      <Wins data={data} onDiscard={onDiscard} phone={phone} />

      <Section title="Macs">
        {macs.length === 0 ? (
          <Row title="No Macs yet" subtitle="Macs you create show up here with what they use." />
        ) : (
          macs.map((mac) => <MacStorageRow key={mac.instance_id} mac={mac} phone={phone} onInfo={() => setInfo(mac)} />)
        )}
      </Section>

      {phone ? (
        <Section>
          <Row title="Storage Limits" value={quotaLabel(data.limits.quota_gb)} onClick={() => setLimits(true)} chevron />
        </Section>
      ) : null}
      <Sheet open={limits} onOpenChange={setLimits} title="Storage Limits" dismissLabel="Done">
        <Limits data={data} />
      </Sheet>
      <Sheet open={!!info} onOpenChange={(open) => !open && setInfo(null)} title={shown?.name ?? ''} dismissLabel="Done">
        {shown ? <Breakdown mac={shown} onDiscard={(mac) => { setInfo(null); onDiscard(mac); }} /> : null}
      </Sheet>
    </div>
  );
}

function Summary({ data, onLimits }: { data: MyStorage; onLimits?: () => void }) {
  const cap = capacity(data.used_bytes, data.limits.quota_gb);
  const segments = kindSegments(data);
  const caption = cap.quota === null ? `${bytes(cap.used)} used, no limit` : `${bytes(cap.used)} of ${bytes(cap.quota)} used`;
  return (
    <Section>
      <CellBody>
        <div className={styles.summary}>
          <div className={styles.summaryLine}>
            <h2 className={styles.volume}>Your Macs</h2>
            <p className={`tabular ${cap.over > 0 ? styles.overText : 'secondary'} ${styles.usedLine}`}>
              {cap.quota === null ? `${bytes(cap.used) || '0 GB'} used` : cap.over > 0 ? `${bytes(cap.over)} over your ${bytes(cap.quota)} limit` : `${bytes(cap.used) || '0 GB'} of ${bytes(cap.quota)} used`}
            </p>
            {onLimits ? (
              <Button variant="gray" size="small" onClick={onLimits}>
                Limits…
              </Button>
            ) : null}
          </div>
          {cap.used > 0 ? (
            <>
              <CapacityBar segments={segments} scale={cap.scale} caption={caption} mark={cap.over > 0 && cap.quota !== null ? { at: cap.quota, label: 'Your limit' } : undefined} />
              <Legend segments={segments} extra={cap.over > 0 ? <li className={styles.limitKey}><span className={styles.limitTick} aria-hidden="true" />Your limit</li> : null} />
            </>
          ) : (
            <p className="t-subhead secondary">Nothing stored yet.</p>
          )}
        </div>
      </CellBody>
    </Section>
  );
}

const WIN_ICON: Record<Win['kind'], IconName> = { memory: 'moon', snapshots: 'stack', files: 'folder' };

/**
 * Like the recommendations in macOS Storage: one row per way to free space,
 * each with its button. Snapshots on several Macs are one row with a list.
 */
function Wins({ data, onDiscard, phone }: { data: MyStorage; onDiscard: (mac: MacStorage) => void; phone: boolean }) {
  const [listing, setListing] = useState(false);
  // iPhone: the whole row is the action, as in iPhone Storage; Mac: a button at the trailing edge.
  const act = (label: string, run: () => void) => (phone ? { onClick: run, chevron: true } : { trailing: <Button variant="gray" size="small" onClick={run}>{label}</Button> });
  const local = data.macs.filter(isLocal);
  const withSnapshots = local.filter((mac) => mac.snapshot_count > 0 && mac.snapshots_bytes > 0).sort((a, b) => b.snapshots_bytes - a.snapshots_bytes);
  const snapshotBytes = withSnapshots.reduce((total, mac) => total + mac.snapshots_bytes, 0);
  const snapshotCount = withSnapshots.reduce((total, mac) => total + mac.snapshot_count, 0);
  const memory = local.filter((mac) => mac.state === 'hibernated' && mac.memory_bytes > 0).sort((a, b) => b.memory_bytes - a.memory_bytes);
  const biggest = local.filter((mac) => mac.disk_bytes > 0).sort((a, b) => b.disk_bytes - a.disk_bytes)[0];
  if (!withSnapshots.length && !memory.length && !biggest) return null;
  const tile = (kind: Win['kind']) => (
    <GlyphTile color={kind === 'memory' ? 'indigo' : kind === 'snapshots' ? 'blue' : 'gray'}>
      <Icon name={WIN_ICON[kind]} size={16} weight={2} />
    </GlyphTile>
  );
  const one = withSnapshots.length === 1 ? withSnapshots[0]! : null;
  return (
    <Section title="Recommendations">
      {withSnapshots.length ? (
        <Row
          leading={tile('snapshots')}
          title="Review snapshots"
          subtitle={`${count(snapshotCount, 'snapshot')}${one ? ` on ${one.name}` : ` on ${count(withSnapshots.length, 'Mac')}`} · ${bytes(snapshotBytes)}`}
          {...act('Review', () => (one ? navigate(macPath(one.instance_id, 'snapshots')) : setListing(true)))}
        />
      ) : null}
      {memory.map((mac) => (
        <Row key={mac.instance_id} leading={tile('memory')} title="Discard saved memory" subtitle={`${mac.name} · ${bytes(mac.memory_bytes)}`} {...act('Discard…', () => onDiscard(mac))} />
      ))}
      {biggest ? (
        <Row leading={tile('files')} title="Clean up files" subtitle={`${biggest.name} · ${bytes(biggest.disk_bytes)}`} {...act('Open Files', () => navigate(macPath(biggest.instance_id, 'files')))} />
      ) : null}
      <Sheet open={listing} onOpenChange={setListing} title="Snapshots" dismissLabel="Done">
        <Section>
          {withSnapshots.map((mac) => (
            <Row key={mac.instance_id} title={mac.name} subtitle={count(mac.snapshot_count, 'snapshot')} value={bytes(mac.snapshots_bytes)} href={macPath(mac.instance_id, 'snapshots')} />
          ))}
        </Section>
      </Sheet>
    </Section>
  );
}

function macSubtitle(mac: MacStorage): string {
  if (!isLocal(mac)) return 'Stored on its own Mac';
  const parts = [`${bytes(mac.disk_bytes) || '0 GB'} disk`];
  if (mac.snapshot_count) parts.push(`${bytes(mac.snapshots_bytes)} in snapshots`);
  if (mac.memory_bytes > 0) parts.push(`${bytes(mac.memory_bytes)} memory`);
  return parts.join(' · ');
}

/** One Mac: its screen, what it keeps, and ⓘ for the details (a pushed page on a phone). */
function MacStorageRow({ mac, phone, onInfo }: { mac: MacStorage; phone: boolean; onInfo: () => void }) {
  const leading = (
    <GlyphTile color={isLocal(mac) ? 'blue' : 'gray'}>
      <Icon name={isLocal(mac) ? 'display' : 'globe'} size={16} weight={2} />
    </GlyphTile>
  );
  if (!isLocal(mac)) return <Row leading={leading} title={mac.name} subtitle="Stored on its own Mac" href={macPath(mac.instance_id)} />;
  if (phone) return <Row leading={leading} title={mac.name} value={bytes(mac.used_bytes) || '0 GB'} href={`/storage?mac=${encodeURIComponent(mac.instance_id)}`} />;
  return (
    <Row
      leading={leading}
      title={mac.name}
      subtitle={macSubtitle(mac)}
      trailing={
        <span className={styles.rowEnd}>
          <span className="tabular secondary">{bytes(mac.used_bytes) || '0 GB'}</span>
          <button type="button" className={styles.info} onClick={onInfo} aria-label={`Details for ${mac.name}`}>
            <Icon name="info" size={17} weight={1.8} />
          </button>
        </span>
      }
    />
  );
}

/** A Mac's details sheet on desktop: the three things it keeps, each with what frees it. */
function Breakdown({ mac, onDiscard }: { mac: MacStorage; onDiscard: (mac: MacStorage) => void }) {
  const hibernated = mac.state === 'hibernated' && mac.memory_bytes > 0;
  const segments = macSegments(mac);
  return (
    <>
      {mac.used_bytes > 0 ? (
        <div className={styles.sheetBar}>
          <CapacityBar segments={segments} scale={mac.used_bytes} caption={`${mac.name} uses ${bytes(mac.used_bytes)}`} />
        </div>
      ) : null}
      <Section>
        <Row
          leading={<Swatch tone="disk" />}
          title="macOS Disk"
          subtitle={`${bytes(mac.disk_bytes) || '0 GB'} of ${diskLabel(mac.disk_gb)}`}
          trailing={<Button variant="gray" size="small" onClick={() => navigate(macPath(mac.instance_id, 'files'))}>Open Files</Button>}
        />
        <Row
          leading={<Swatch tone="snapshots" />}
          title="Snapshots"
          subtitle={mac.snapshot_count ? `${bytes(mac.snapshots_bytes)} in ${count(mac.snapshot_count, 'snapshot')}` : 'None'}
          trailing={<Button variant="gray" size="small" onClick={() => navigate(macPath(mac.instance_id, 'snapshots'))}>Manage</Button>}
        />
        <Row
          leading={<Swatch tone="memory" />}
          title="Saved Memory"
          subtitle={hibernated ? bytes(mac.memory_bytes) : 'None'}
          trailing={hibernated ? <Button variant="gray" size="small" onClick={() => onDiscard(mac)}>Discard…</Button> : undefined}
        />
      </Section>
    </>
  );
}

/** Phone: the pushed screen for one Mac, in the iPhone Storage idiom. */
function MacDetail({ mac, onDiscard }: { mac: MacStorage; onDiscard: (mac: MacStorage) => void }) {
  const segments = macSegments(mac);
  const hibernated = mac.state === 'hibernated' && mac.memory_bytes > 0;
  return (
    <div className={styles.stack}>
      <Section>
        <CellBody>
          <div className={styles.summary}>
            <p className={styles.used}>
              <span className={`tabular ${styles.usedValue}`}>{bytes(mac.used_bytes) || '0 GB'}</span>
              <span className="secondary"> on the server</span>
            </p>
            {mac.used_bytes > 0 ? (
              <>
                <CapacityBar segments={segments} scale={mac.used_bytes} caption={`${mac.name} uses ${bytes(mac.used_bytes)}`} />
                <Legend segments={segments} />
              </>
            ) : null}
          </div>
        </CellBody>
      </Section>
      <Section title="macOS Disk" footer={`Grows as the Mac writes files, up to ${diskLabel(mac.disk_gb)}.`}>
        <Row title="Used" value={`${bytes(mac.disk_bytes)} of ${diskLabel(mac.disk_gb)}`} />
        <Row title="Open Files" tone="tint" href={macPath(mac.instance_id, 'files')} />
      </Section>
      <Section title="Snapshots" footer="Delete the ones you no longer need.">
        <Row title={mac.snapshot_count ? count(mac.snapshot_count, 'snapshot') : 'No snapshots'} value={mac.snapshot_count ? bytes(mac.snapshots_bytes) : undefined} />
        <Row title="Manage Snapshots" tone="tint" href={macPath(mac.instance_id, 'snapshots')} />
      </Section>
      <Section title="Saved Memory" footer={hibernated ? 'Kept so the Mac resumes where it left off.' : 'Only kept while a Mac hibernates.'}>
        <Row title={hibernated ? 'Saved memory' : 'None saved'} value={hibernated ? bytes(mac.memory_bytes) : undefined} />
        {hibernated ? <Row title="Discard Saved Memory…" tone="destructive" onClick={() => onDiscard(mac)} chevron={false} /> : null}
      </Section>
    </div>
  );
}

/** Only a limit chosen for this person is worth a note; defaults go unsaid. */
const mine = (source: Parameters<typeof sourceLabel>[0]) => (source === 'override' ? sourceLabel(source) : undefined);

function Limits({ data }: { data: MyStorage }) {
  const { data: session } = useSession();
  const admin = session?.user?.role === 'admin';
  const footer = admin ? (
    <>
      Change limits in{' '}
      <a href="/admin/storage" onClick={(event) => onLinkClick(event)}>
        Storage Limits
      </a>
      .
    </>
  ) : (
    'Need more room? Ask an administrator.'
  );
  return (
    <Section footer={footer}>
      <Row title="Storage limit" subtitle={mine(data.sources.quota_gb)} value={quotaLabel(data.limits.quota_gb)} />
      <Row title="Largest disk for a new Mac" subtitle={mine(data.sources.max_disk_gb)} value={diskLabel(data.limits.max_disk_gb)} />
      <Row title="Snapshots per Mac" subtitle={mine(data.sources.max_snapshots)} value={snapshotsLabel(data.limits.max_snapshots)} />
    </Section>
  );
}

function DiscardMemory({ mac, open, onClose }: { mac: MacStorage | null; open: boolean; onClose: () => void }) {
  async function discard() {
    if (!mac) return;
    try {
      const instance = await api.discardMemory(mac.instance_id);
      setData(keys.instance(instance.id), instance);
      updateData<Instance[]>(keys.instances, (list) => list.map((item) => (item.id === instance.id ? instance : item)));
      updateData<MyStorage>(keys.storage, (data) => {
        const macs = data.macs.map((item) => (item.instance_id === instance.id ? { ...item, state: instance.state, memory_bytes: 0, used_bytes: item.used_bytes - item.memory_bytes } : item));
        const used = data.used_bytes - mac.memory_bytes;
        return { ...data, macs, used_bytes: used, over_quota: data.limits.quota_gb > 0 && used >= data.limits.quota_gb * GB };
      });
      invalidate(keys.storage);
      notify.success(`Discarded ${bytes(mac.memory_bytes)} of saved memory`);
    } catch (cause) {
      notify.error(cause);
    }
  }
  return (
    <Confirm
      open={open && !!mac}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title={`Discard the saved memory of ${mac?.name ?? 'this Mac'}?`}
      message={`${mac?.name ?? 'The Mac'} will start fresh the next time you turn it on, as if it had been shut down. Anything unsaved in its open apps is lost. This frees ${bytes(mac?.memory_bytes ?? 0)}.`}
      action="Discard Memory"
      onConfirm={() => withUndo(`Discarding saved memory of ${mac?.name ?? 'the Mac'}…`, discard)}
    />
  );
}
