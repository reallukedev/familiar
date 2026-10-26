import { useState } from 'react';
import { ApprovalList } from '../components/Approvals';
import { Banner, CountBadge, StatusLabel } from '../components/Bits';
import { Button, IconButton } from '../components/Button';
import { Icon } from '../components/Icon';
import { Row, Section } from '../components/List';
import { MacScreen } from '../components/MacScreen';
import { SkeletonBlock, SkeletonRows } from '../components/Skeleton';
import { EmptyState } from '../components/Bits';
import { Familiar } from '../components/Familiar';
import { api } from '../lib/api';
import { keys, useQuery } from '../lib/cache';
import { whenInline } from '../lib/format';
import { usePlatform } from '../lib/platform';
import { navigate, onLinkClick } from '../lib/router';
import { macPath } from '../lib/routes';
import { useSession } from '../lib/session';
import { instanceInfo, isLockedOff, stateDetail } from '../lib/status';
import { unlockMacs } from '../lib/unlock';
import { RecoveryKeyPrompt } from '../components/RecoveryKeySheet';
import type { Approval, Instance } from '../lib/types';
import { usePower } from './mac/usePower';
import { Page } from '../shells/Page';
import { NewMacSheet } from './NewMac';
import styles from './Macs.module.css';
import { PowerConfirm } from './mac/PowerMenu';

/** The Mac to feature: one needing attention, else the most recently used. */
export function pickHero(instances: Instance[]): Instance | null {
  if (!instances.length) return null;
  const score = (instance: Instance) => (instance.counts.pending_approvals ? 2 : 0) + (instance.state === 'ready' ? 1 : 0);
  return instances.toSorted((a, b) => score(b) - score(a) || (b.last_active_at ?? '').localeCompare(a.last_active_at ?? ''))[0] ?? null;
}

export function Macs() {
  const phone = usePlatform() === 'phone';
  const { data: session } = useSession();
  const instances = useQuery<Instance[]>(keys.instances, api.instances, { pollMs: 15_000 });
  const [creating, setCreating] = useState(false);
  const list = instances.data;
  const hero = list ? pickHero(list) : null;
  const others = list?.filter((instance) => instance.id !== hero?.id) ?? [];
  const firstName = session?.user?.display_name.split(' ')[0] ?? 'My';
  const suggested = list?.length ? `Mac ${list.length + 1}` : `${firstName}’s Mac`;

  const addButton = phone ? (
    <IconButton label="New Mac" onClick={() => setCreating(true)}>
      <Icon name="plus" size={22} weight={2.2} />
    </IconButton>
  ) : (
    <Button variant="gray" icon={<Icon name="plus" size={14} weight={2.4} />} onClick={() => setCreating(true)}>
      New Mac
    </Button>
  );

  return (
    <Page title={list && list.length === 1 ? 'Your Mac' : 'Your Macs'} width="wide" actions={list?.length ? addButton : undefined}>
      {instances.error && !list ? (
        <EmptyState icon="wifi" title="Couldn’t load your Macs" action={<Button variant="tinted" onClick={() => void instances.reload()}>Try Again</Button>}>
          {instances.error.message}
        </EmptyState>
      ) : !list ? (
        <div className={styles.loading} role="status" aria-label="Loading your Macs">
          <SkeletonBlock height="min(52vw, 520px)" radius={18} />
          <SkeletonRows rows={2} leading />
        </div>
      ) : list.length === 0 ? (
        <FirstMac onCreate={() => setCreating(true)} />
      ) : (
        <>
          <RecoveryKeyPrompt />
          <LockedMacs instances={list} />
          <PendingApprovals instances={list} />
          {hero ? <Hero instance={hero} /> : null}
          {others.length && !phone ? (
            <section className={styles.others} aria-labelledby="other-macs">
              <h2 className={`t-title3 ${styles.othersTitle}`} id="other-macs">
                Other Macs
              </h2>
              <ul className={styles.grid}>
                {others.map((instance) => (
                  <MacTile key={instance.id} instance={instance} />
                ))}
              </ul>
            </section>
          ) : others.length ? (
            <Section title="Other Macs">
              {others.map((instance) => (
                <MacRow key={instance.id} instance={instance} />
              ))}
            </Section>
          ) : null}
        </>
      )}
      <NewMacSheet open={creating} onOpenChange={setCreating} suggestedName={suggested} />
    </Page>
  );
}

function FirstMac({ onCreate }: { onCreate: () => void }) {
  return (
    <div className={styles.first}>
      <div className={styles.blank} aria-hidden="true">
        <div className={styles.blankScreen}>
          <Familiar state="idle" size={104} variant="glyph" />
        </div>
      </div>
      <div className={styles.firstText}>
        <h2 className="t-title2 balance">Your own Mac, for your assistant</h2>
        <p className="t-body secondary pretty">
          Create a private Mac on this server. Your assistant can see its screen, use its apps and get things done on it, and only you can watch, step in or pause it.
        </p>
        <div>
          <Button variant="filled" size="large" onClick={onCreate}>
            Create Your Mac
          </Button>
        </div>
      </div>
    </div>
  );
}

/** After a server restart: one calm line for every locked Mac, with the way to unlock them. */
function LockedMacs({ instances }: { instances: Instance[] }) {
  const locked = instances.filter(isLockedOff);
  if (!locked.length) return null;
  const one = locked.length === 1;
  return (
    <div className={styles.locked}>
      <Banner
        tone="gray"
        icon="lock"
        title={one ? `${locked[0]!.name} is locked` : `${locked.length} Macs are locked`}
        action={
          <Button variant="filled" size="small" onClick={() => void unlockMacs()}>
            Unlock…
          </Button>
        }
      >
        Familiar restarted. Sign in to unlock {one ? 'it' : 'them'}.
      </Banner>
    </div>
  );
}

function PendingApprovals({ instances }: { instances: Instance[] }) {
  const waiting = instances.filter((instance) => instance.counts.pending_approvals > 0);
  if (!waiting.length) return null;
  return (
    <section className={styles.approvals} aria-labelledby="waiting-for-you">
      <h2 className={`t-title3 ${styles.othersTitle}`} id="waiting-for-you">
        Waiting for You
      </h2>
      <div className={styles.approvalList}>
        {waiting.map((instance) => (
          <MacApprovals key={instance.id} instance={instance} showName={instances.length > 1} />
        ))}
      </div>
    </section>
  );
}

function MacApprovals({ instance, showName }: { instance: Instance; showName: boolean }) {
  const approvals = useQuery<Approval[]>(keys.approvals(instance.id), () => api.approvals(instance.id));
  return <ApprovalList approvals={approvals.data ?? []} macName={showName ? instance.name : undefined} />;
}

function Hero({ instance }: { instance: Instance }) {
  const info = instanceInfo(instance);
  const power = usePower(instance);
  const href = macPath(instance.id);
  return (
    <section className={styles.hero} aria-labelledby={`hero-${instance.id}`}>
      <a href={href} onClick={(event) => onLinkClick(event)} className={styles.heroDisplay} aria-label={`Open ${instance.name}`}>
        <MacScreen instance={instance} size="hero" />
      </a>
      <div className={styles.heroText}>
        <div className={styles.heroTitle}>
          <h2 className="t-title2" id={`hero-${instance.id}`}>
            <a href={href} onClick={(event) => onLinkClick(event)} className={styles.heroLink}>
              {instance.name}
            </a>
          </h2>
          <StatusLabel tone={info.tone} pulse={info.transitional}>
            {info.label}
            {instance.paused ? ', assistant paused' : ''}
          </StatusLabel>
        </div>
        <p className="t-subhead secondary pretty">
          {instance.state === 'ready' ? (instance.last_active_at ? `Last used ${whenInline(instance.last_active_at)}` : null) : stateDetail(instance)}
        </p>
        <div className={styles.heroActions}>
          {isLockedOff(instance) ? (
            <Button variant="filled" icon={<Icon name="lock" size={14} weight={2.2} />} onClick={() => void unlockMacs()}>
              Unlock…
            </Button>
          ) : info.primary ? (
            <Button variant="filled" onClick={() => power.run(info.primary!.action)} busy={!!power.busy}>
              {info.primary.label}
            </Button>
          ) : null}
          <Button variant={info.primary || isLockedOff(instance) ? 'gray' : 'filled'} onClick={() => navigate(href)}>
            Open {instance.name.length > 18 ? 'Mac' : instance.name}
          </Button>
        </div>
      </div>
      <PowerConfirm instance={instance} power={power} />
    </section>
  );
}

/** Desktop: a Mac as a small display with its name under it, like Screen Sharing's grid. */
function MacTile({ instance }: { instance: Instance }) {
  const info = instanceInfo(instance);
  const waiting = instance.counts.pending_approvals;
  return (
    <li>
      <a href={macPath(instance.id)} onClick={(event) => onLinkClick(event)} className={styles.tile}>
        <span className={styles.tileDisplay}>
          <MacScreen instance={instance} size="card" />
        </span>
        <span className={styles.tileText}>
          <span className={styles.tileName}>{instance.name}</span>
          <StatusLabel tone={info.tone} pulse={info.transitional}>
            {info.label}
            {instance.paused ? ' · Assistant paused' : ''}
          </StatusLabel>
        </span>
        {waiting ? (
          <span className={styles.tileBadge}>
            <CountBadge count={waiting} label={`${waiting} waiting for approval`} />
          </span>
        ) : null}
      </a>
    </li>
  );
}

function MacRow({ instance }: { instance: Instance }) {
  const info = instanceInfo(instance);
  return (
    <Row
      href={macPath(instance.id)}
      title={instance.name}
      subtitle={
        <StatusLabel tone={info.tone} pulse={info.transitional}>
          {info.label}
          {instance.paused ? ' · Assistant paused' : ''}
        </StatusLabel>
      }
      leading={
        <span className={styles.rowDisplay}>
          <MacScreen instance={instance} size="tiny" />
        </span>
      }
      trailing={<CountBadge count={instance.counts.pending_approvals} label={`${instance.counts.pending_approvals} waiting for approval`} />}
    />
  );
}
