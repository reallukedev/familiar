import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ApprovalList } from '../../components/Approvals';
import { ActivityRow } from '../../components/ActivityRow';
import { EmptyState, Pill } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Row, Section } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { SkeletonRows } from '../../components/Skeleton';
import { Spinner } from '../../components/Spinner';
import { api } from '../../lib/api';
import { fetchKey, keys, setData, useEntry, useQuery } from '../../lib/cache';
import { dayHeading, stamp, when } from '../../lib/format';
import { macPath } from '../../lib/routes';
import { notify } from '../../lib/toast';
import type { Activity, ActivityPage as Page_, Approval } from '../../lib/types';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Activity.module.css';

type Filter = 'all' | 'error' | 'waiting';

function groupByDay(items: Activity[], now: number) {
  const groups: { heading: string; items: Activity[] }[] = [];
  for (const item of items) {
    const heading = dayHeading(item.at, now);
    const last = groups.at(-1);
    if (last && last.heading === heading) last.items.push(item);
    else groups.push({ heading, items: [item] });
  }
  return groups;
}

export function ActivityPage({ instance, title = 'Activity', back, actions }: MacPageProps & { title?: string; back?: { href: string; label: string } | null; actions?: ReactNode }) {
  const [filter, setFilter] = useState<Filter>('all');
  const pending = instance.counts.pending_approvals;
  return (
    <Page title={title} back={back === null ? undefined : (back ?? { href: macPath(instance.id), label: instance.name })} actions={actions} width="wide">
      <div className={styles.filters}>
        <Segmented
          label="Show"
          value={filter}
          onChange={setFilter}
          options={[
            { value: 'all', label: 'All' },
            { value: 'error', label: 'Errors' },
            { value: 'waiting', label: pending ? `Waiting (${pending})` : 'Waiting', hint: 'Waiting for approval' },
          ]}
        />
      </div>
      {filter === 'waiting' ? <Waiting instanceId={instance.id} /> : <Feed instanceId={instance.id} filter={filter} />}
    </Page>
  );
}

function Feed({ instanceId, filter }: { instanceId: string; filter: 'all' | 'error' }) {
  const key = keys.activity(instanceId, filter);
  const fetchFirst = () => api.activity(instanceId, { limit: 50, outcome: filter === 'error' ? 'error' : undefined });
  const page = useQuery<Page_>(key, fetchFirst);
  const fresh = useEntry<string[]>(keys.fresh(instanceId)).data;
  const [loadingMore, setLoadingMore] = useState(false);
  const sentinel = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);

  // Highlighted rows fade back once they have been seen for a moment.
  useEffect(() => {
    if (!fresh?.length) return;
    const timer = setTimeout(() => setData(keys.fresh(instanceId), []), 2500);
    return () => clearTimeout(timer);
  }, [fresh, instanceId]);

  const next = page.data?.next ?? null;
  async function loadMore() {
    if (!next || loadingMore) return;
    setLoadingMore(true);
    try {
      const more = await api.activity(instanceId, { before: next, limit: 50, outcome: filter === 'error' ? 'error' : undefined });
      const current = page.data;
      if (current) {
        const seen = new Set(current.items.map((item) => item.id));
        setData<Page_>(key, { items: [...current.items, ...more.items.filter((item) => !seen.has(item.id))], next: more.next });
      }
    } catch (error) {
      notify.error(error);
    } finally {
      setLoadingMore(false);
    }
  }

  useEffect(() => {
    const node = sentinel.current;
    if (!node || !next) return;
    const observer = new IntersectionObserver(([entry]) => entry?.isIntersecting && void loadMore(), { rootMargin: '400px' });
    observer.observe(node);
    return () => observer.disconnect();
  });

  if (page.error && !page.data) {
    return (
      <EmptyState icon="wifi" title="Couldn’t load activity" action={<Button variant="tinted" onClick={() => void fetchKey(key, fetchFirst).catch(() => {})}>Try Again</Button>}>
        {page.error.message}
      </EmptyState>
    );
  }
  if (!page.data) return <div className={styles.loading}><SkeletonRows rows={6} label="Loading activity" /></div>;
  if (!page.data.items.length) {
    return filter === 'error' ? (
      <EmptyState icon="check" title="No errors">Requests that fail show up here with the reason.</EmptyState>
    ) : (
      <EmptyState familiar="idle" title="No activity yet">When your assistant uses this Mac, every request shows up here as it happens: what it did, how long it took and whether it worked.</EmptyState>
    );
  }

  return (
    <div className={styles.list} aria-live="polite" aria-relevant="additions">
      {groupByDay(page.data.items, now).map((group) => (
        <Section key={group.heading} title={group.heading}>
          <ul>
            {group.items.map((item) => (
              <ActivityRow key={item.id} item={item} now={now} fresh={fresh?.includes(item.id)} />
            ))}
          </ul>
        </Section>
      ))}
      <div ref={sentinel} className={styles.more}>
        {next ? (
          loadingMore ? (
            <Spinner size={20} label="Loading more activity" />
          ) : (
            <Button variant="plain" onClick={() => void loadMore()}>
              Load Earlier Activity
            </Button>
          )
        ) : (
          <p className="t-footnote secondary">That’s everything Familiar keeps for this Mac.</p>
        )}
      </div>
    </div>
  );
}

function Waiting({ instanceId }: { instanceId: string }) {
  const approvals = useQuery<Approval[]>(keys.approvals(instanceId), () => api.approvals(instanceId));
  if (!approvals.data) return approvals.error ? <EmptyState icon="wifi" title="Couldn’t load approvals">{approvals.error.message}</EmptyState> : <div className={styles.loading}><SkeletonRows rows={3} /></div>;
  const pending = approvals.data.filter((item) => item.status === 'pending');
  const past = approvals.data.filter((item) => item.status !== 'pending');
  const now = Date.now();
  return (
    <>
      {pending.length ? (
        <div className={styles.pending}>
          <ApprovalList approvals={pending} />
        </div>
      ) : (
        <EmptyState familiar="idle" title="Nothing waiting">When a permission is set to Ask, your assistant’s request waits here for up to a minute until you allow or deny it.</EmptyState>
      )}
      {past.length ? (
        <Section title="Earlier requests">
          {past.map((item) => (
            <Row
              key={item.id}
              align="top"
              title={item.title}
              subtitle={`${item.summary}${item.client ? ` · ${item.client}` : ''}`}
              value={<span title={stamp(item.created_at)}>{when(item.created_at, now)}</span>}
              trailing={<Pill tone={item.status === 'approved' ? 'green' : item.status === 'denied' ? 'red' : 'gray'}>{item.status === 'approved' ? 'Allowed' : item.status === 'denied' ? 'Denied' : 'Expired'}</Pill>}
            />
          ))}
        </Section>
      ) : null}
    </>
  );
}
