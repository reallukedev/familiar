import { useState, type FormEvent } from 'react';
import { EmptyState } from '../../components/Bits';
import { Button, IconButton } from '../../components/Button';
import { TextArea } from '../../components/Field';
import { Icon, type IconName } from '../../components/Icon';
import { GlyphTile, Row, Section } from '../../components/List';
import { ActionMenu } from '../../components/Menu';
import { Segmented } from '../../components/Segmented';
import { Sheet } from '../../components/Sheet';
import { SkeletonRows } from '../../components/Skeleton';
import { api } from '../../lib/api';
import { invalidate, keys, updateData, useQuery } from '../../lib/cache';
import { count, stamp, when, whenInline } from '../../lib/format';
import { usePlatform } from '../../lib/platform';
import { macPath } from '../../lib/routes';
import { notify } from '../../lib/toast';
import type { Instance, ReviewItem } from '../../lib/types';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Review.module.css';

const NOTE_MAX = 1000;

type Tab = 'pending' | 'done';
type Group = { key: string; client: string; session: string | null; items: ReviewItem[]; latest: string };

/** Pending changes by assistant session, newest session first; each session reads in order. */
export function groupBySession(items: ReviewItem[]): Group[] {
  const groups = new Map<string, Group>();
  for (const item of items) {
    const key = item.session ?? item.connection_id ?? item.client ?? 'unknown';
    let group = groups.get(key);
    if (!group) groups.set(key, (group = { key, client: item.client ?? 'Your assistant', session: item.session, items: [], latest: item.created_at }));
    group.items.push(item);
    if (item.created_at > group.latest) group.latest = item.created_at;
  }
  const list = [...groups.values()];
  for (const group of list) group.items.sort((a, b) => a.created_at.localeCompare(b.created_at));
  return list.sort((a, b) => b.latest.localeCompare(a.latest));
}

/** A glyph for what kind of change a tool makes. */
function toolIcon(tool: string): { icon: IconName; color: 'blue' | 'indigo' | 'green' | 'orange' | 'red' | 'gray' } {
  if (/^(mail|messages)_|_send$/.test(tool)) return { icon: 'envelope', color: 'red' };
  if (/^(files?_|mac_file|finder_)/.test(tool)) return { icon: 'doc', color: 'orange' };
  if (/shell|script|terminal/.test(tool)) return { icon: 'terminal', color: 'gray' };
  if (/^safari_|_url$/.test(tool)) return { icon: 'globe', color: 'blue' };
  if (/clipboard/.test(tool)) return { icon: 'clipboard', color: 'gray' };
  if (/checkpoint|snapshot/.test(tool)) return { icon: 'stack', color: 'green' };
  if (/install|uninstall|_app$/.test(tool)) return { icon: 'grid', color: 'indigo' };
  if (/^calendar_|^reminders_/.test(tool)) return { icon: 'clock', color: 'red' };
  if (/^notes_/.test(tool)) return { icon: 'doc', color: 'orange' };
  return { icon: 'cursor', color: 'indigo' };
}

export function ReviewPage({ instance }: MacPageProps) {
  const [tab, setTab] = useState<Tab>('pending');
  const pending = instance.counts.pending_review ?? 0;
  const phone = usePlatform() === 'phone';
  const picker = (
    <Segmented
      label="Show"
      size={phone ? 'regular' : 'compact'}
      value={tab}
      onChange={setTab}
      options={[
        { value: 'pending', label: pending ? `To Review (${pending})` : 'To Review' },
        { value: 'done', label: 'Done' },
      ]}
    />
  );
  return (
    // Mac: the switch sits in the toolbar, as in Mail or Reminders; iPhone: under the title.
    <Page title="Review" back={{ href: macPath(instance.id), label: instance.name }} actions={phone ? undefined : <div className={styles.toolbarPicker}>{picker}</div>}>
      {phone ? <div className={styles.filters}>{picker}</div> : null}
      {tab === 'pending' ? <Pending instance={instance} /> : <Done instanceId={instance.id} />}
    </Page>
  );
}

/** After a decision: drop it from the pending list now; the server's event brings the counts. */
function settle(instance: Instance, ids: string[]) {
  updateData<ReviewItem[]>(keys.review(instance.id, 'pending'), (list) => list.filter((item) => !ids.includes(item.id)));
  updateData<Instance>(keys.instance(instance.id), (current) => ({
    ...current,
    counts: { ...current.counts, pending_review: Math.max(0, (current.counts.pending_review ?? 0) - ids.length) },
  }));
  invalidate(`review:${instance.id}:done`);
}

function Pending({ instance }: { instance: Instance }) {
  const list = useQuery<ReviewItem[]>(keys.review(instance.id, 'pending'), () => api.review(instance.id, 'pending'));
  const [declining, setDeclining] = useState<ReviewItem | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const now = Date.now();

  async function accept(item: ReviewItem) {
    setBusy(item.id);
    try {
      await api.decideReview(instance.id, item.id, 'accept');
      settle(instance, [item.id]);
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  async function acceptAll(group: Group) {
    setBusy(group.key);
    const ids = group.items.map((item) => item.id);
    try {
      await api.acceptAllReview(instance.id, { ids });
      settle(instance, ids);
      notify.success(`Accepted ${count(ids.length, 'change')}`);
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  if (list.error && !list.data) {
    return (
      <EmptyState icon="wifi" title="Couldn’t load changes" action={<Button variant="tinted" onClick={() => void list.reload()}>Try Again</Button>}>
        {list.error.message}
      </EmptyState>
    );
  }
  if (!list.data) {
    return (
      <div className={styles.loading}>
        <SkeletonRows rows={4} label="Loading changes" />
      </div>
    );
  }
  if (list.data.length === 0) {
    return (
      <EmptyState familiar="idle" title="Nothing to review">
        Changes your assistant makes without asking show up here.
      </EmptyState>
    );
  }

  const groups = groupBySession(list.data);
  return (
    <>
      {groups.map((group, index) => (
        <Section
          key={group.key}
          title={
            <span className={styles.head}>
              {group.client}
              <span className={styles.headMeta} title={stamp(group.latest)}>
                {when(group.latest, now)} · {count(group.items.length, 'change')}
              </span>
            </span>
          }
          accessory={
            <Button variant="gray" size="small" onClick={() => void acceptAll(group)} busy={busy === group.key} disabled={!!busy && busy !== group.key}>
              Accept All
            </Button>
          }
          footer={index === groups.length - 1 ? 'Declining tells your assistant, with your note, the next time it does something.' : undefined}
        >
          {group.items.map((item) => (
            <ChangeRow key={item.id} item={item} now={now} busy={busy === item.id} locked={!!busy && busy !== item.id} onAccept={() => void accept(item)} onDecline={() => setDeclining(item)} />
          ))}
        </Section>
      ))}
      <DeclineSheet
        instance={instance}
        item={declining}
        onOpenChange={(open) => {
          if (!open) setDeclining(null);
        }}
      />
    </>
  );
}

function ChangeRow({ item, now, busy, locked, onAccept, onDecline }: { item: ReviewItem; now: number; busy: boolean; locked: boolean; onAccept: () => void; onDecline: () => void }) {
  const phone = usePlatform() === 'phone';
  const look = toolIcon(item.tool);
  const leading = (
    <GlyphTile color={look.color}>
      <Icon name={look.icon} size={16} weight={2} />
    </GlyphTile>
  );
  const subtitle = (
    <>
      <span className={styles.summary}>{item.summary}</span>
      <span className={`tabular ${styles.time}`} title={stamp(item.created_at)}>
        {when(item.created_at, now)}
      </span>
    </>
  );
  if (phone) {
    return (
      <Row
        align="top"
        leading={leading}
        title={item.title}
        subtitle={subtitle}
        trailing={
          <ActionMenu
            label={`Review ${item.title}`}
            trigger={
              <IconButton label={`Review ${item.title}`} disabled={busy || locked}>
                <Icon name="ellipsis" size={20} weight={2} />
              </IconButton>
            }
            items={[
              { label: 'Accept', onSelect: onAccept },
              { label: 'Decline…', onSelect: onDecline, tone: 'destructive' },
            ]}
          />
        }
        className={styles.row}
      />
    );
  }
  // Mac: one line per change, like a Mail list. Small ✕ and ✓ wait quietly at the end.
  return (
    <div className={styles.line}>
      <span className={styles.lineIcon}>{leading}</span>
      <span className={styles.lineTitle}>{item.title}</span>
      <span className={styles.lineSummary} title={item.summary}>
        {item.summary}
      </span>
      <span className={`tabular ${styles.lineTime}`} title={stamp(item.created_at)}>
        {when(item.created_at, now)}
      </span>
      <span className={styles.lineActions}>
        <button type="button" className={`${styles.round} ${styles.no}`} onClick={onDecline} disabled={busy || locked} aria-label={`Decline ${item.title}`} title="Decline…">
          <Icon name="xmark" size={11} weight={2.8} />
        </button>
        <button type="button" className={`${styles.round} ${styles.yes}`} onClick={onAccept} disabled={busy || locked} aria-label={`Accept ${item.title}`} title="Accept">
          <Icon name="check" size={12} weight={2.8} />
        </button>
      </span>
    </div>
  );
}

function DeclineSheet({ instance, item, onOpenChange }: { instance: Instance; item: ReviewItem | null; onOpenChange: (open: boolean) => void }) {
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [shown, setShown] = useState<ReviewItem | null>(item);
  // Keep the last item while the sheet animates closed.
  if (item && item !== shown) {
    setShown(item);
    setNote('');
  }
  const tooLong = note.trim().length > NOTE_MAX;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!item || tooLong) return;
    setBusy(true);
    try {
      await api.decideReview(instance.id, item.id, 'decline', note.trim() || undefined);
      settle(instance, [item.id]);
      notify.success(note.trim() ? 'Declined and sent your note' : 'Declined');
      onOpenChange(false);
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={!!item}
      onOpenChange={onOpenChange}
      title="Decline Change"
      description={shown ? `${shown.title}: ${shown.summary}` : undefined}
      primary={{ label: 'Decline', form: 'decline-change', busy, disabled: tooLong, tone: 'destructive' }}
      dismissLabel="Cancel"
    >
      <form id="decline-change" onSubmit={(event) => void submit(event)} className={styles.form}>
        <TextArea
          label="Note for your assistant"
          value={note}
          onValueChange={setNote}
          rows={3}
          maxLength={NOTE_MAX + 200}
          placeholder="Optional"
          error={tooLong ? `Keep it under ${NOTE_MAX} characters.` : null}
          description={`${shown?.client ?? 'Your assistant'} sees this the next time it does something on this Mac.`}
        />
      </form>
    </Sheet>
  );
}

function Done({ instanceId }: { instanceId: string }) {
  const phone = usePlatform() === 'phone';
  const list = useQuery<ReviewItem[]>(keys.review(instanceId, 'done'), () => api.review(instanceId, 'done'));
  const now = Date.now();
  if (list.error && !list.data) {
    return (
      <EmptyState icon="wifi" title="Couldn’t load reviewed changes" action={<Button variant="tinted" onClick={() => void list.reload()}>Try Again</Button>}>
        {list.error.message}
      </EmptyState>
    );
  }
  if (!list.data) {
    return (
      <div className={styles.loading}>
        <SkeletonRows rows={4} label="Loading reviewed changes" />
      </div>
    );
  }
  if (list.data.length === 0) {
    return (
      <EmptyState icon="check" title="Nothing reviewed yet">
        Changes you accept or decline are kept here.
      </EmptyState>
    );
  }
  if (!phone) {
    return (
      <Section>
        {list.data.map((item) => {
          const declined = item.status === 'declined';
          const who = item.client ?? 'your assistant';
          return (
            <div key={item.id} className={styles.line}>
              <span className={`${styles.verdict} ${declined ? styles.declined : styles.accepted}`} aria-label={declined ? 'Declined' : 'Accepted'}>
                <Icon name={declined ? 'xmark' : 'check'} size={11} weight={2.8} />
              </span>
              <span className={styles.lineTitle}>{item.title}</span>
              <span className={styles.lineSummary} title={item.note ? `${item.summary}\nNote: ${item.note}` : item.summary}>
                {item.note ? <span className={styles.lineNote}>“{item.note}”</span> : item.summary}
              </span>
              <span className={`tabular ${styles.lineTime}`} title={stamp(item.decided_at)}>
                {when(item.decided_at, now)}
              </span>
              <span className={styles.lineStatus}>{declined ? (item.delivered_at ? `${who} was told` : 'Not seen yet') : ''}</span>
            </div>
          );
        })}
      </Section>
    );
  }
  return (
    <Section>
      {list.data.map((item) => {
        const declined = item.status === 'declined';
        const who = item.client ?? 'your assistant';
        const told = declined ? (item.delivered_at ? `${who} was told` : `${who} hasn’t seen it yet`) : null;
        return (
          <Row
            key={item.id}
            align="top"
            leading={
              <span className={`${styles.verdict} ${declined ? styles.declined : styles.accepted}`} aria-hidden="true">
                <Icon name={declined ? 'xmark' : 'check'} size={13} weight={2.8} />
              </span>
            }
            title={item.title}
            subtitle={
              <>
                <span className={styles.summary}>{item.summary}</span>
                {item.note ? <span className={styles.note}>“{item.note}”</span> : null}
                <span className={styles.time} title={stamp(item.decided_at)}>
                  {declined ? 'Declined' : 'Accepted'} {whenInline(item.decided_at, now)}
                  {told ? ` · ${told}` : ''}
                </span>
              </>
            }
            className={styles.row}
          />
        );
      })}
    </Section>
  );
}
