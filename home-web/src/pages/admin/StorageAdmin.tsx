import { useState, type ReactNode } from 'react';
import { Avatar, Banner, EmptyState } from '../../components/Bits';
import { Button } from '../../components/Button';
import { CellBody, Row, Section } from '../../components/List';
import { SkeletonBlock, SkeletonRows } from '../../components/Skeleton';
import { Switch } from '../../components/Switch';
import { CapacityBar, Legend } from '../../components/storage/CapacityBar';
import { LimitInput } from '../../components/storage/LimitInput';
import { PersonLimitsSheet } from '../../components/storage/PersonLimitsSheet';
import { ApiError, api } from '../../lib/api';
import { keys, peek, setData, useQuery } from '../../lib/cache';
import { bytes, count, whenInline } from '../../lib/format';
import { usePlatform } from '../../lib/platform';
import { useSession } from '../../lib/session';
import {
  GB, LIMIT_KEYS, capacity, commitment, diskLabel, draftFromDefaults, parseWhole, quotaLabel, readDefaults, serverSegments, underReserve,
  type DefaultsDraft, type DraftErrors, type LimitKey,
} from '../../lib/storage';
import { notify } from '../../lib/toast';
import type { AdminStorage, PersonStorage, StorageDefaults, StorageOverrides } from '../../lib/types';
import { Page } from '../../shells/Page';
import styles from './StorageAdmin.module.css';

const TITLE = 'Storage Limits';
const back = { href: '/admin', label: 'Server' };

/** Server › Storage Limits: how the server's disk is used, and how much each person may use. */
export function StorageAdmin() {
  const query = useQuery<AdminStorage>(keys.adminStorage, api.adminStorage, { pollMs: 60_000 });
  const data = query.data;
  return (
    <Page title={TITLE} back={back} width="wide">
      {query.error && !data ? (
        <EmptyState icon="wifi" title="Couldn’t load storage" action={<Button variant="tinted" onClick={() => void query.reload()}>Try Again</Button>}>
          {query.error.message}
        </EmptyState>
      ) : !data ? (
        <Loading />
      ) : (
        <Loaded data={data} />
      )}
    </Page>
  );
}

function Loading() {
  return (
    <div className={styles.page} role="status" aria-label="Loading storage">
      <Section>
        <CellBody>
          <SkeletonBlock height={104} radius={8} />
        </CellBody>
      </Section>
      <div className={`${styles.defaults} ${styles.loadingDefaults}`}>
        <SkeletonRows rows={2} />
        <SkeletonRows rows={2} />
      </div>
      <Section>
        <SkeletonRows rows={4} leading />
      </Section>
    </div>
  );
}

/** What the server would answer, worked out here so a save shows at once. */
function withDefaults(admin: AdminStorage, defaults: AdminStorage['defaults']): AdminStorage {
  return withPeople({ ...admin, defaults }, (person) => person);
}

function withPeople(admin: AdminStorage, change: (person: PersonStorage) => PersonStorage): AdminStorage {
  const people = admin.people.map((person) => {
    const next = change(person);
    const limits = {
      quota_gb: next.overrides.quota_gb ?? admin.defaults.quota_gb,
      max_disk_gb: next.overrides.max_disk_gb ?? admin.defaults.max_disk_gb,
      max_snapshots: next.overrides.max_snapshots ?? admin.defaults.max_snapshots,
    };
    return { ...next, limits, over_quota: limits.quota_gb > 0 && next.used_bytes >= limits.quota_gb * GB };
  });
  const unlimited = people.some((person) => person.limits.quota_gb <= 0);
  return { ...admin, people, committed_gb: unlimited ? 0 : people.reduce((sum, person) => sum + person.limits.quota_gb, 0) };
}

function Loaded({ data }: { data: AdminStorage }) {
  const phone = usePlatform() === 'phone';
  // Kept after closing so the sheet's text doesn't change while it animates out.
  const [editing, setEditing] = useState<{ id: string | null; open: boolean }>({ id: null, open: false });
  const [personError, setPersonError] = useState<{ key: LimitKey; message: string } | null>(null);
  const person = data.people.find((item) => item.user_id === editing.id) ?? null;

  async function savePerson(target: PersonStorage, body: Partial<StorageOverrides>): Promise<boolean> {
    const before = peek<AdminStorage>(keys.adminStorage);
    setPersonError(null);
    if (before) setData(keys.adminStorage, withPeople(before, (item) => (item.user_id === target.user_id ? { ...item, overrides: { ...item.overrides, ...body } } : item)));
    try {
      setData(keys.adminStorage, await api.setPersonStorage(target.user_id, body));
      notify.success(`Saved limits for ${target.display_name}`);
      return true;
    } catch (error) {
      if (before) setData(keys.adminStorage, before);
      if (error instanceof ApiError && error.field && (LIMIT_KEYS as readonly string[]).includes(error.field)) {
        setPersonError({ key: error.field as LimitKey, message: error.message });
      }
      notify.error(error);
      return false;
    }
  }

  return (
    <div className={styles.page}>
      <Alerts data={data} />
      <Disk data={data} />
      <Defaults data={data} />
      <People data={data} phone={phone} onEdit={(item) => {
        setPersonError(null);
        setEditing({ id: item.user_id, open: true });
      }} />
      <PersonLimitsSheet
        person={person}
        defaults={data.defaults}
        open={editing.open}
        onOpenChange={(open) => setEditing((value) => ({ ...value, open }))}
        onSave={savePerson}
        serverError={personError}
      />
    </div>
  );
}

// ---- The server's disk ----

function nearlyFull(data: AdminStorage): boolean {
  const reserve = data.defaults.reserve_gb * GB;
  return data.disk_free_bytes < Math.max(reserve * 2, data.disk_total_bytes * 0.1);
}

function Alerts({ data }: { data: AdminStorage }) {
  const promise = commitment(data);
  const reserve = bytes(data.defaults.reserve_gb * GB);
  const banners: ReactNode[] = [];
  if (underReserve(data)) {
    banners.push(
      <Banner key="reserve" tone="red" icon="warning" title="The server is almost out of space">
        Less than {reserve} is free, so nobody can create Macs or take snapshots. Macs that are running keep going. Ask people to free space, or delete macOS images you no longer need.
      </Banner>,
    );
  } else if (nearlyFull(data)) {
    banners.push(
      <Banner key="full" tone="orange" icon="warning" title={`The server disk is nearly full: ${bytes(data.disk_free_bytes)} available`}>
        When less than {reserve} is free, nobody can create Macs or take snapshots until space is freed.
      </Banner>,
    );
  }
  if (promise.kind === 'over') {
    banners.push(
      <Banner key="promise" tone="orange" icon="info" title="Limits add up to more than the disk can hold">
        Limits allow {bytes(promise.committed)}; the disk holds about {bytes(promise.room)}. Fine while people stay under their limits.
      </Banner>,
    );
  }
  if (!banners.length) return null;
  return <div className={styles.banners}>{banners}</div>;
}

function Disk({ data }: { data: AdminStorage }) {
  const segments = serverSegments(data);
  const total = data.disk_total_bytes;
  const used = Math.max(0, total - data.disk_free_bytes);
  const reserveAt = Math.max(0, total - data.defaults.reserve_gb * GB);
  const low = underReserve(data);
  return (
    <Section
      title="Server disk"
      footer={<>The line marks the reserve. Measured {whenInline(data.measured_at)}.</>}
    >
      <CellBody>
        <div className={styles.summary}>
          <div className={styles.summaryLine}>
            <p className={styles.used}>
              <span className={`tabular ${styles.usedValue}`}>{bytes(used) || '0 GB'}</span>
              <span className="secondary"> of {bytes(total)} used</span>
            </p>
            <p className={`t-subhead tabular ${low ? styles.redText : 'secondary'}`}>{bytes(data.disk_free_bytes) || '0 GB'} available</p>
          </div>
          <CapacityBar segments={segments} scale={total} caption={`Server disk: ${bytes(used)} of ${bytes(total)} used`} mark={{ at: reserveAt, label: 'Reserve' }} />
          <Legend
            segments={segments}
            extra={
              <li className={styles.limitKey}>
                <span className={styles.limitTick} aria-hidden="true" />
                Reserve <span className="secondary tabular">{bytes(data.defaults.reserve_gb * GB)}</span>
              </li>
            }
          />
        </div>
      </CellBody>
    </Section>
  );
}

// ---- Server-wide limits ----

type Field = keyof StorageDefaults;
const FIELDS: Field[] = ['quota_gb', 'max_snapshots', 'default_disk_gb', 'max_disk_gb', 'reserve_gb'];

function same(a: DefaultsDraft, b: DefaultsDraft): boolean {
  if (a.quota_unlimited !== b.quota_unlimited || a.snapshots_unlimited !== b.snapshots_unlimited) return false;
  const text = (draft: DefaultsDraft, key: 'quota_gb' | 'max_snapshots' | 'default_disk_gb' | 'max_disk_gb' | 'reserve_gb') => draft[key].replace(/[,\s]/g, '');
  return (
    (a.quota_unlimited || text(a, 'quota_gb') === text(b, 'quota_gb')) &&
    (a.snapshots_unlimited || text(a, 'max_snapshots') === text(b, 'max_snapshots')) &&
    text(a, 'default_disk_gb') === text(b, 'default_disk_gb') &&
    text(a, 'max_disk_gb') === text(b, 'max_disk_gb') &&
    text(a, 'reserve_gb') === text(b, 'reserve_gb')
  );
}

function Defaults({ data }: { data: AdminStorage }) {
  const saved = draftFromDefaults(data.defaults);
  // null while nothing is being edited, so a refresh from the server shows through.
  const [draft, setDraft] = useState<DefaultsDraft | null>(null);
  const [touched, setTouched] = useState<Set<Field>>(new Set());
  const [attempted, setAttempted] = useState(false);
  const [serverErrors, setServerErrors] = useState<DraftErrors>({});
  const [saving, setSaving] = useState(false);

  const shown = draft ?? saved;
  // The server-wide snapshot cap rides along with the other defaults.
  const savedTotal = data.defaults.snapshot_total_gb ?? 0;
  const [totalDraft, setTotalDraft] = useState<{ unlimited: boolean; text: string } | null>(null);
  const [totalTouched, setTotalTouched] = useState(false);
  const [totalServerError, setTotalServerError] = useState<string | undefined>();
  const total = totalDraft ?? { unlimited: savedTotal <= 0, text: savedTotal > 0 ? String(savedTotal) : '' };
  const typedTotal = parseWhole(total.text);
  const totalValue = total.unlimited ? 0 : typedTotal !== null && typedTotal >= 1 && typedTotal <= 100_000 ? typedTotal : null;
  const totalChanged = totalDraft !== null && totalValue !== savedTotal;
  const totalError = totalServerError ?? ((attempted || totalTouched) && totalValue === null ? 'Use 1 to 100,000 GB, or turn on No Limit.' : undefined);
  const dirty = saving || (draft !== null && !same(draft, saved)) || totalChanged;
  const result = readDefaults(shown, data.defaults);
  const errors: DraftErrors = 'errors' in result ? result.errors : {};
  const errorFor = (key: Field) => serverErrors[key] ?? (attempted || touched.has(key) ? errors[key] : undefined);

  function edit(patch: Partial<DefaultsDraft>, key: Field) {
    setDraft({ ...shown, ...patch });
    setServerErrors((prev) => {
      const { [key]: _gone, ...rest } = prev;
      return rest;
    });
  }
  const blur = (key: Field) => setTouched((prev) => new Set(prev).add(key));

  function revert() {
    setDraft(null);
    setTotalDraft(null);
    setTotalTouched(false);
    setTotalServerError(undefined);
    setTouched(new Set());
    setAttempted(false);
    setServerErrors({});
  }

  async function save() {
    if (saving) return;
    setAttempted(true);
    if ('errors' in result) {
      const first = FIELDS.find((key) => result.errors[key]);
      if (first) document.getElementById(`defaults-${first}`)?.focus();
      return;
    }
    if (totalValue === null) {
      document.getElementById('defaults-snapshot_total_gb')?.focus();
      return;
    }
    const body: Parameters<typeof api.setStorageDefaults>[0] = { ...result.body, ...(totalChanged ? { snapshot_total_gb: totalValue } : {}) };
    if (!Object.keys(body).length) {
      revert();
      return;
    }
    const before = peek<AdminStorage>(keys.adminStorage) ?? data;
    setSaving(true);
    setData(keys.adminStorage, withDefaults(before, { ...before.defaults, ...body }));
    try {
      setData(keys.adminStorage, await api.setStorageDefaults(body));
      revert();
      notify.success('Saved the limits for everyone');
    } catch (error) {
      setData(keys.adminStorage, before);
      if (error instanceof ApiError && error.field === 'snapshot_total_gb') setTotalServerError(error.message);
      if (error instanceof ApiError && error.field && (FIELDS as string[]).includes(error.field)) {
        setServerErrors({ [error.field as Field]: error.message });
      }
      notify.error(error);
    } finally {
      setSaving(false);
    }
  }

  const input = (key: Exclude<Field, never>, value: string, onValue: (text: string) => void, unit?: string) => {
    const error = errorFor(key);
    return (
      <LimitInput
        id={`defaults-${key}`}
        value={value}
        onValueChange={onValue}
        onBlur={() => blur(key)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') void save();
        }}
        unit={unit}
        invalid={!!error}
        disabled={saving}
        aria-labelledby={`defaults-${key}-label`}
        aria-describedby={error ? `defaults-${key}-error` : undefined}
      />
    );
  };
  const title = (key: Field, text: string) => <span id={`defaults-${key}-label`}>{text}</span>;
  const subtitle = (key: Field, hint?: string) => {
    const error = errorFor(key);
    if (error) {
      return (
        <span id={`defaults-${key}-error`} className={styles.redText}>
          {error}
        </span>
      );
    }
    return hint;
  };

  return (
    <div className={styles.defaultsBlock}>
      <div className={styles.defaultsHead}>
        <h2 className="t-title2">Limits for everyone</h2>
        <p className="t-subhead secondary">Lowering a limit never deletes anything, apart from old snapshots.</p>
      </div>
      <form
        className={styles.defaults}
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <Section
          title="Storage per person"
          footer={
            shown.quota_unlimited
              ? 'Up to what the server has free.'
              : 'At the limit, people can’t create Macs or take snapshots.'
          }
        >
          <Row title="No limit" trailing={<Switch label="No storage limit" checked={shown.quota_unlimited} onCheckedChange={(on) => edit({ quota_unlimited: on, quota_gb: on ? shown.quota_gb : shown.quota_gb || '500' }, 'quota_gb')} disabled={saving} />} />
          {shown.quota_unlimited ? null : (
            <Row align={errorFor('quota_gb') ? 'top' : 'center'} title={title('quota_gb', 'Limit')} subtitle={subtitle('quota_gb')} trailing={input('quota_gb', shown.quota_gb, (text) => edit({ quota_gb: text }, 'quota_gb'), 'GB')} />
          )}
        </Section>

        <Section
          title="Snapshots per Mac"
          footer={
            shown.snapshots_unlimited
              ? 'As many as their storage allows.'
              : undefined
          }
        >
          <Row title="No limit" trailing={<Switch label="No snapshot limit" checked={shown.snapshots_unlimited} onCheckedChange={(on) => edit({ snapshots_unlimited: on, max_snapshots: on ? shown.max_snapshots : shown.max_snapshots || '10' }, 'max_snapshots')} disabled={saving} />} />
          {shown.snapshots_unlimited ? null : (
            <Row align={errorFor('max_snapshots') ? 'top' : 'center'} title={title('max_snapshots', 'Snapshots')} subtitle={subtitle('max_snapshots')} trailing={input('max_snapshots', shown.max_snapshots, (text) => edit({ max_snapshots: text }, 'max_snapshots'))} />
          )}
        </Section>

        <Section
          title="Snapshots on the server"
          footer={total.unlimited ? 'As much as the server has free.' : 'Past the limit, the oldest snapshots are deleted first. Ones people mark Keep stay.'}
        >
          <Row title="No limit" trailing={<Switch label="No limit on snapshots on the server" checked={total.unlimited} onCheckedChange={(on) => setTotalDraft({ unlimited: on, text: on ? total.text : total.text || '500' })} disabled={saving} />} />
          {total.unlimited ? null : (
            <Row
              align={totalError ? 'top' : 'center'}
              title={<span id="defaults-snapshot_total_gb-label">All snapshots</span>}
              subtitle={
                totalError ? (
                  <span id="defaults-snapshot_total_gb-error" className={styles.redText}>
                    {totalError}
                  </span>
                ) : undefined
              }
              trailing={
                <LimitInput
                  id="defaults-snapshot_total_gb"
                  value={total.text}
                  onValueChange={(text) => {
                    setTotalDraft({ unlimited: false, text });
                    setTotalServerError(undefined);
                  }}
                  onBlur={() => setTotalTouched(true)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') void save();
                  }}
                  unit="GB"
                  invalid={!!totalError}
                  disabled={saving}
                  aria-labelledby="defaults-snapshot_total_gb-label"
                  aria-describedby={totalError ? 'defaults-snapshot_total_gb-error' : undefined}
                />
              }
            />
          )}
        </Section>

        <Section title="New Macs" footer="Macs only use what they write.">
          <Row
            align={errorFor('default_disk_gb') ? 'top' : 'center'}
            title={title('default_disk_gb', 'Disk size')}
            subtitle={subtitle('default_disk_gb', 'Unless the owner picks another.')}
            trailing={input('default_disk_gb', shown.default_disk_gb, (text) => edit({ default_disk_gb: text }, 'default_disk_gb'), 'GB')}
          />
          <Row
            align={errorFor('max_disk_gb') ? 'top' : 'center'}
            title={title('max_disk_gb', 'Largest disk')}
            subtitle={subtitle('max_disk_gb', 'The most anyone can pick.')}
            trailing={input('max_disk_gb', shown.max_disk_gb, (text) => edit({ max_disk_gb: text }, 'max_disk_gb'), 'GB')}
          />
        </Section>

        <Section title="Server reserve" footer="Below this, no one can create Macs or take snapshots.">
          <Row
            align={errorFor('reserve_gb') ? 'top' : 'center'}
            title={title('reserve_gb', 'Keep free')}
            subtitle={subtitle('reserve_gb')}
            trailing={input('reserve_gb', shown.reserve_gb, (text) => edit({ reserve_gb: text }, 'reserve_gb'), 'GB')}
          />
        </Section>

        {dirty ? (
          <div className={styles.saveBar}>
            <p className="t-subhead secondary">{saving ? 'Saving…' : 'Unsaved changes'}</p>
            <div className={styles.saveButtons}>
              <Button variant="gray" onClick={revert} disabled={saving}>
                Revert
              </Button>
              <Button variant="filled" type="submit" busy={saving}>
                Save
              </Button>
            </div>
          </div>
        ) : null}
      </form>
    </div>
  );
}

// ---- People ----

/** "446 GB", or "None" before anything is stored. */
function size(value: number): string {
  return value > 0 ? bytes(value) : 'None';
}

/** What else is set for this person besides their storage limit: "Disks up to 128 GB · 5 snapshots". */
function otherLimits(person: PersonStorage): string {
  const parts: string[] = [];
  if (person.overrides.max_disk_gb !== null) parts.push(`Disks up to ${diskLabel(person.overrides.max_disk_gb)}`);
  if (person.overrides.max_snapshots !== null) parts.push(person.overrides.max_snapshots > 0 ? count(person.overrides.max_snapshots, 'snapshot') : 'Any number of snapshots');
  return parts.join(' · ');
}

function sortPeople(people: PersonStorage[]): PersonStorage[] {
  return people.toSorted((a, b) => b.used_bytes - a.used_bytes || a.display_name.localeCompare(b.display_name));
}

type Standing = { tone: 'over' | 'near' | 'ok'; text: string };

function standing(person: PersonStorage): Standing {
  const cap = capacity(person.used_bytes, person.limits.quota_gb);
  if (cap.quota === null) return { tone: 'ok', text: 'No limit' };
  if (cap.over > 0) return { tone: 'over', text: `${bytes(cap.over)} over` };
  if (person.over_quota) return { tone: 'over', text: 'At the limit' };
  if (person.used_bytes >= cap.quota * 0.9) return { tone: 'near', text: `${bytes(cap.available) || '0 GB'} left` };
  return { tone: 'ok', text: `${bytes(cap.available)} left` };
}

function custom(person: PersonStorage): boolean {
  return LIMIT_KEYS.some((key) => person.overrides[key] !== null);
}

function UsageBar({ person }: { person: PersonStorage }) {
  const cap = capacity(person.used_bytes, person.limits.quota_gb);
  if (cap.quota === null) return <span className={styles.barSpace} aria-hidden="true" />;
  return (
    <span className={styles.bar}>
      <CapacityBar
        size="thin"
        segments={person.used_bytes > 0 ? [{ key: 'used', label: 'Used', bytes: person.used_bytes, tone: 'disk' }] : []}
        scale={cap.scale}
        caption=""
        mark={cap.over > 0 ? { at: cap.quota, label: 'Limit' } : undefined}
      />
    </span>
  );
}

function PeopleFooter({ data }: { data: AdminStorage }) {
  const promise = commitment(data);
  const over = data.people.filter((person) => person.over_quota).length;
  const parts: string[] = [];
  if (over) parts.push(`${count(over, 'person is', 'people are')} at their limit.`);
  if (promise.kind === 'unbounded' && promise.unlimited > 0) {
    parts.push(promise.unlimited === data.people.length ? 'No one has a storage limit.' : `${count(promise.unlimited, 'person has', 'people have')} no storage limit.`);
  } else if (promise.kind === 'fits') {
  }
  return <>{parts.join(' ')}</>;
}

function People({ data, phone, onEdit }: { data: AdminStorage; phone: boolean; onEdit: (person: PersonStorage) => void }) {
  const { data: session } = useSession();
  const me = session?.user?.id;
  const people = sortPeople(data.people);
  const you = (person: PersonStorage) => (person.user_id === me ? <span className="secondary"> (you)</span> : null);

  if (phone) {
    return (
      <Section title="People" footer={<PeopleFooter data={data} />}>
        {people.map((person) => {
          const state = standing(person);
          const cap = capacity(person.used_bytes, person.limits.quota_gb);
          const used = cap.quota === null ? (person.used_bytes > 0 ? `${bytes(person.used_bytes)} used` : 'Nothing stored') : `${person.used_bytes > 0 ? bytes(person.used_bytes) : '0 GB'} of ${bytes(cap.quota)}`;
          return (
            <Row
              key={person.user_id}
              onClick={() => onEdit(person)}
              chevron
              label={`${person.display_name}, ${used}${state.tone === 'ok' ? '' : `, ${state.text}`}. Edit limits`}
              leading={<Avatar name={person.display_name} size="small" />}
              title={
                <span className={styles.phoneTitle}>
                  <span className={styles.phoneName}>
                    {person.display_name}
                    {you(person)}
                  </span>
                  <UsageBar person={person} />
                </span>
              }
              subtitle={
                <>
                  <span className={state.tone === 'over' ? styles.redText : state.tone === 'near' ? styles.orangeText : undefined}>{state.tone === 'ok' ? used : `${used} · ${state.text}`}</span>
                  {` · ${count(person.mac_count, 'Mac')}${custom(person) ? ' · Own limits' : ''}`}
                </>
              }
            />
          );
        })}
      </Section>
    );
  }

  return (
    <Section title="People" footer={<PeopleFooter data={data} />}>
      <table className={styles.table}>
        <thead>
          <tr>
            <th scope="col" className={styles.colPerson}>Person</th>
            <th scope="col" className={styles.colUsed}>Used</th>
            <th scope="col" className={styles.colLimit}>Limit</th>
            <th scope="col" className={styles.colMacs}>Macs</th>
            <th scope="col" className={styles.colEdit}>
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {people.map((person) => {
            const state = standing(person);
            return (
              <tr key={person.user_id} data-tone={state.tone}>
                <th scope="row" className={styles.colPerson}>
                  <span className={styles.person}>
                    <Avatar name={person.display_name} size="small" />
                    <span className={styles.personText}>
                      <span className={styles.name} title={person.display_name}>
                        {person.display_name}
                        {you(person)}
                      </span>
                      <span className={`secondary ${styles.sub}`} title={person.username}>
                        {person.username} · {person.role === 'admin' ? 'Admin' : 'Member'}
                      </span>
                    </span>
                  </span>
                </th>
                <td className={styles.colUsed}>
                  <span className={styles.usedCell}>
                    <UsageBar person={person} />
                    <span className={styles.usedText}>
                      <span className="tabular">{size(person.used_bytes)}</span>
                      {state.tone === 'ok' ? null : <span className={`tabular ${styles.sub} ${state.tone === 'over' ? styles.redText : styles.orangeText}`}>{state.text}</span>}
                    </span>
                  </span>
                </td>
                <td className={styles.colLimit}>
                  <span className={styles.stacked}>
                    <span className="tabular">{quotaLabel(person.limits.quota_gb)}</span>
                    <span className={`secondary ${styles.sub}`} title={otherLimits(person) || undefined}>
                      {[person.overrides.quota_gb !== null ? 'Set for them' : 'Default', otherLimits(person)].filter(Boolean).join(' · ')}
                    </span>
                  </span>
                </td>
                <td className={`tabular ${styles.colMacs}`}>{person.mac_count}</td>
                <td className={styles.colEdit}>
                  <Button variant="gray" size="small" onClick={() => onEdit(person)} aria-label={`Edit limits for ${person.display_name}`}>
                    Edit…
                  </Button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </Section>
  );
}
