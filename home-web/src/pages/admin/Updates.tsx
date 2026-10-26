import { useEffect, useOptimistic, useState, useTransition, type ReactNode } from 'react';
import { EmptyState, Pill } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Choice, type ChoiceOption } from '../../components/Choice';
import { Confirm } from '../../components/Confirm';
import { Icon } from '../../components/Icon';
import { CellBody, Row, Section } from '../../components/List';
import { ProgressBar } from '../../components/Meter';
import { SkeletonRows } from '../../components/Skeleton';
import { Spinner } from '../../components/Spinner';
import { Switch } from '../../components/Switch';
import { api } from '../../lib/api';
import { keys, setData, useEntry, useQuery } from '../../lib/cache';
import { count, day, stamp, when, whenInline } from '../../lib/format';
import { ADMIN_STREAM, useAdminLive, type LiveState } from '../../lib/live';
import { usePlatform } from '../../lib/platform';
import { watchRestart } from '../../lib/restart';
import { useStore } from '../../lib/store';
import { notify } from '../../lib/toast';
import { CHANNEL_DETAIL, CHANNEL_NAME, noteLines, offer, restart, shortCommit, versionLabel, windowRange, type RestartState } from '../../lib/updates';
import type { Channel, UpdateEvent, UpdatesStatus } from '../../lib/types';
import { Page } from '../../shells/Page';
import styles from './Updates.module.css';
import { withUndo } from '../../lib/undo';

type Settings = UpdatesStatus['settings'];
type Busy = 'install' | 'check' | 'rollback' | null;

const CHANNELS: ChoiceOption<Channel | 'off'>[] = (['stable', 'nightly', 'off'] as const).map((value) => ({ value, label: CHANNEL_NAME[value], detail: CHANNEL_DETAIL[value] }));
const AGENT_CHANNELS: ChoiceOption<Channel | 'off'>[] = [
  { value: 'stable', label: 'Stable', detail: CHANNEL_DETAIL.stable },
  { value: 'nightly', label: 'Nightly', detail: CHANNEL_DETAIL.nightly },
  { value: 'off', label: 'Off', detail: 'Agents stay on the version they have.' },
];
const HOURS: ChoiceOption<string>[] = Array.from({ length: 24 }, (_, hour) => ({ value: String(hour), label: windowRange(hour) }));
const HISTORY_SHOWN = 6;

/** Admin > Software Update, after System Settings > General > Software Update. */
export function SoftwareUpdate() {
  const status = useQuery<UpdatesStatus>(keys.updates, api.updates);
  useAdminLive(true);
  const back = { href: '/admin', label: 'Server' };
  return (
    <Page title="Software Update" back={back}>
      {status.error && !status.data ? (
        <EmptyState icon="wifi" title="Couldn’t load update status" action={<Button variant="tinted" onClick={() => void status.reload()}>Try Again</Button>}>
          {status.error.message}
        </EmptyState>
      ) : !status.data ? (
        <div className={styles.gap}>
          <SkeletonRows rows={4} label="Loading update status" />
        </div>
      ) : (
        <Loaded status={status.data} />
      )}
    </Page>
  );
}

function Loaded({ status }: { status: UpdatesStatus }) {
  const phone = usePlatform() === 'phone';
  const restarting = useStore(restart);
  const live = useEntry<LiveState>(keys.live(ADMIN_STREAM)).data;
  const [busy, setBusy] = useState<Busy>(null);
  const [confirmRollback, setConfirmRollback] = useState(false);
  const [settings, setOptimistic] = useOptimistic(status.settings, (prev: Settings, patch: Partial<Settings>) => ({ ...prev, ...patch }));
  const [, startTransition] = useTransition();

  // The server goes away while it restarts. Wait for it when it says so, or
  // when the stream drops in the middle of an install.
  const midInstall = status.state === 'downloading' || status.state === 'installing';
  useEffect(() => {
    if (status.state === 'restarting' || (midInstall && live === 'reconnecting')) watchRestart(status.current.version);
  }, [status.state, midInstall, live, status.current.version]);

  async function run(kind: Exclude<Busy, null>) {
    setBusy(kind);
    try {
      const next = kind === 'install' ? await api.installUpdate() : kind === 'check' ? await api.checkUpdates() : await api.rollbackUpdate();
      setData(keys.updates, next);
      if (kind === 'rollback') watchRestart(status.current.version);
      if (kind === 'check' && next.state === 'idle' && offer(next).kind !== 'newer' && offer(next).kind !== 'switch') notify.info('Familiar is up to date');
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  const save = (patch: Partial<Settings>) =>
    startTransition(async () => {
      setOptimistic(patch);
      try {
        setData(keys.updates, await api.setUpdates(patch));
      } catch (error) {
        notify.error(error);
      }
    });

  const working = restarting.phase === 'waiting' || status.state === 'downloading' || status.state === 'installing' || status.state === 'restarting';
  const enabled = status.enabled;
  const off = settings.channel === 'off';

  return (
    <>
      <Section>
        <StatusCard status={status} restarting={restarting} busy={busy} onInstall={() => void run('install')} />
        {enabled && !off && !working ? (
          phone ? (
            <Row
              title="Check Now"
              tone="tint"
              onClick={() => void run('check')}
              disabled={busy === 'check' || status.state === 'checking'}
              subtitle={busy === 'check' || status.state === 'checking' ? 'Checking…' : status.last_checked_at ? `Last checked ${whenInline(status.last_checked_at)}` : 'Not checked yet'}
              chevron={false}
            />
          ) : (
            <Row
              title="Last checked"
              value={status.last_checked_at ? <time dateTime={status.last_checked_at} title={stamp(status.last_checked_at)}>{when(status.last_checked_at)}</time> : 'Never'}
              trailing={
                <Button variant="gray" size="small" onClick={() => void run('check')} busy={busy === 'check' || status.state === 'checking'}>
                  Check Now
                </Button>
              }
            />
          )
        ) : null}
      </Section>

      {enabled ? (
        <>
          <Section
            title="Familiar"
            footer={
              off
                ? 'Familiar won’t look for new versions until you choose a channel.'
                : settings.auto_install
                  ? 'Macs keep running while it restarts.'
                  : 'Macs keep running while it restarts.'
            }
          >
            <Row title="Update channel" subtitle={CHANNEL_DETAIL[settings.channel]} trailing={<Choice label="Update channel" value={settings.channel} options={CHANNELS} onChange={(channel) => save({ channel })} disabled={working} />} />
            {off ? null : (
              <Row title="Install updates automatically" trailing={<Switch checked={settings.auto_install} onCheckedChange={(on) => save({ auto_install: on })} label="Install updates automatically" disabled={working} />} />
            )}
            {!off && settings.auto_install ? (
              <Row title="Install window" subtitle="In the server’s time zone." trailing={<Choice label="Install window" value={String(settings.window_start_hour)} options={HOURS} onChange={(hour) => save({ window_start_hour: Number(hour) })} disabled={working} />} />
            ) : null}
          </Section>

          <Section title="Mac agents" footer="Each Mac can choose its own in Settings.">
            <Row title="Mac agents follow" subtitle={settings.guest_default_channel === 'off' ? 'Agents stay on the version they have.' : CHANNEL_DETAIL[settings.guest_default_channel]} trailing={<Choice label="Mac agents follow" value={settings.guest_default_channel} options={AGENT_CHANNELS} onChange={(channel) => save({ guest_default_channel: channel })} disabled={working} />} />
            <Row title="Update agents automatically" subtitle="While their Mac is idle." trailing={<Switch checked={settings.guest_auto_install} onCheckedChange={(on) => save({ guest_auto_install: on })} label="Update agents automatically" disabled={working} />} />
          </Section>

          {status.previous && !working ? (
            <Section footer="Your Macs keep running.">
              <Row title={`Roll Back to ${status.previous.version}…`} tone="destructive" onClick={() => setConfirmRollback(true)} disabled={busy === 'rollback'} chevron={false} />
            </Section>
          ) : null}
        </>
      ) : null}

      <History events={status.history} />

      <Confirm
        open={confirmRollback}
        onOpenChange={setConfirmRollback}
        title={`Roll back to Familiar ${status.previous?.version ?? ''}?`}
        message="Familiar restarts on the earlier version. Your Macs keep running, and you can update again later."
        action="Roll Back"
        onConfirm={() => withUndo('Rolling back…', () => run('rollback'))}
      />
    </>
  );
}

function StatusCard({ status, restarting, busy, onInstall }: { status: UpdatesStatus; restarting: RestartState; busy: Busy; onInstall: () => void }) {
  const current = versionLabel(status.current.version, status.current.commit, status.current.channel);
  const channel = status.current.channel ? CHANNEL_NAME[status.current.channel] : null;
  const available = status.available;
  const next = offer(status);
  const target = available?.version ?? '';

  let title: string;
  let lines: ReactNode = null;
  let action: ReactNode = null;
  let progress = false;
  let tone: 'default' | 'warn' | 'error' = 'default';
  let showNotes = false;
  let busyAction = false;

  if (restarting.phase === 'waiting') {
    title = 'Restarting Familiar…';
    lines = restarting.slow
      ? 'This is taking longer than usual. The page keeps trying. If Familiar doesn’t come back in a few minutes, check the server’s log.'
      : 'Your Macs keep running. This page reconnects on its own.';
    action = <Spinner size={22} label="Waiting for Familiar" />;
    busyAction = true;
  } else if (!status.enabled) {
    title = 'Updates aren’t available for this build';
    lines = (
      <>
        <span>{status.disabled_reason ?? 'This build can’t check for updates.'}</span>
        <span>Familiar {current}</span>
      </>
    );
  } else if (status.state === 'downloading' || status.state === 'installing' || status.state === 'restarting') {
    title = status.state === 'downloading' ? `Downloading Familiar ${target}` : status.state === 'installing' ? `Installing Familiar ${target}` : 'Restarting Familiar…';
    lines = status.detail ?? 'Your Macs keep running.';
    progress = status.state !== 'restarting';
    if (status.state === 'restarting') {
      action = <Spinner size={22} label="Restarting" />;
      busyAction = true;
    }
  } else if (status.state === 'checking') {
    title = 'Checking for updates…';
    lines = `Familiar ${current}${channel ? ` · ${channel}` : ''}`;
    action = <Spinner size={22} label="Checking for updates" />;
    busyAction = true;
  } else if (status.settings.channel === 'off') {
    title = 'Updates are off';
    lines = `Familiar ${current}. Choose a channel below to check for new versions again.`;
  } else if (next.kind === 'newer' || next.kind === 'switch' || status.state === 'failed') {
    const failed = status.state === 'failed';
    const switching = next.kind === 'switch';
    title = failed ? 'The update didn’t install' : switching ? `${CHANNEL_NAME[next.channel]} has Familiar ${target}` : `Familiar ${target} is available`;
    tone = failed ? 'error' : switching ? 'warn' : 'default';
    lines = failed ? (
      <span>{status.detail ?? `Familiar is still on ${current}.`}</span>
    ) : switching ? (
      <span>It’s older than the nightly build you run now ({status.current.version}). Installing it moves Familiar back to the last tested release.</span>
    ) : (
      <span>
        You have {current}.{available ? <> Released <time dateTime={available.published_at}>{day(available.published_at)}</time>.</> : null}
      </span>
    );
    showNotes = !failed;
    if (next.kind === 'newer' || next.kind === 'switch') {
      action = (
        <Button variant="filled" onClick={onInstall} busy={busy === 'install'}>
          {failed ? 'Try Again' : switching ? `Install ${CHANNEL_NAME[next.channel]}` : 'Update Now'}
        </Button>
      );
    }
  } else {
    title = 'Familiar is up to date';
    lines = `Familiar ${current}${channel ? ` · ${channel}` : ''}`;
  }

  return (
    <CellBody className={styles.body}>
      <div className={styles.card}>
        <img className={styles.appIcon} src="/icon.svg" alt="" width={56} height={56} />
        <div className={styles.summary} aria-live="polite">
          <h2 className={`t-headline ${styles.title}`}>{title}</h2>
          {lines ? <p className={`t-subhead ${tone === 'warn' ? 'text-orange' : tone === 'error' ? 'text-red' : 'secondary'} ${styles.lines}`}>{lines}</p> : null}
          {progress ? (
            <div className={styles.progress}>
              <ProgressBar value={null} label={title} />
            </div>
          ) : null}
        </div>
        {action ? <div className={`${styles.action} ${busyAction ? styles.spinning : ''}`}>{action}</div> : null}
      </div>
      {showNotes && available ? <Notes notes={available.notes} url={available.url} nightly={status.settings.channel === 'nightly'} commit={available.commit} /> : null}
    </CellBody>
  );
}

const LONG_NOTES = 8;

function Notes({ notes, url, nightly, commit }: { notes: string | null; url: string | null; nightly: boolean; commit: string }) {
  const [expanded, setExpanded] = useState(false);
  const lines = noteLines(notes);
  const long = lines.length > LONG_NOTES;
  if (!lines.length && !url) return null;
  const shown = long && !expanded ? lines.slice(0, LONG_NOTES - 1) : lines;
  return (
    <div className={styles.notes}>
      {/* Stable notes bring their own headings; nightly notes are commit subjects. */}
      <h3 className={nightly ? `t-subhead ${styles.heading}` : 'visually-hidden'}>{nightly ? `Changes in build ${shortCommit(commit)}` : 'Release notes'}</h3>
      {lines.length ? (
        <div className={`t-subhead ${styles.notesText}`} id="release-notes">
          {shown.map((line, i) => (line.text ? <p key={i} className={line.heading ? styles.heading : undefined}>{line.text}</p> : <p key={i} className={styles.blank} aria-hidden="true" />))}
        </div>
      ) : null}
      <div className={styles.notesLinks}>
        {long ? (
          <button type="button" className={styles.more} onClick={() => setExpanded((open) => !open)} aria-expanded={expanded} aria-controls="release-notes">
            {expanded ? 'Show Less' : 'Show More'}
          </button>
        ) : null}
        {url ? (
          <a className={styles.more} href={url} target="_blank" rel="noopener noreferrer">
            Open Release Page
            <Icon name="arrow-up-right" size={13} weight={2.2} />
          </a>
        ) : null}
      </div>
    </div>
  );
}

const OUTCOME: Record<UpdateEvent['outcome'], { label: string; tone: 'green' | 'orange' | 'red' }> = {
  installed: { label: 'Installed', tone: 'green' },
  rolled_back: { label: 'Rolled back', tone: 'orange' },
  failed: { label: 'Failed', tone: 'red' },
};

function History({ events }: { events: UpdateEvent[] }) {
  const [all, setAll] = useState(false);
  if (!events.length) return null;
  const shown = all ? events : events.slice(0, HISTORY_SHOWN);
  return (
    <Section title="History">
      {shown.map((event) => {
        const outcome = OUTCOME[event.outcome];
        return (
          <Row
            key={event.id}
            align="top"
            title={event.target_name}
            subtitle={
              <>
                <span className={styles.versions}>
                  {event.from_version ? (
                    <>
                      {event.from_version}
                      <Icon name="chevron-right" size={11} weight={2.6} className={styles.arrow} label="to" />
                    </>
                  ) : null}
                  {event.to_version}
                  <span aria-hidden="true"> · </span>
                  <time dateTime={event.at} title={stamp(event.at)}>
                    <span className="visually-hidden">, </span>
                    {when(event.at)}
                  </time>
                </span>
                {event.detail ? <span className={styles.detail}>{event.detail}</span> : null}
              </>
            }
            trailing={<Pill tone={outcome.tone}>{outcome.label}</Pill>}
          />
        );
      })}
      {events.length > HISTORY_SHOWN ? <Row title={all ? 'Show Fewer' : `Show All ${count(events.length, 'Update')}`} tone="tint" onClick={() => setAll((open) => !open)} chevron={false} /> : null}
    </Section>
  );
}
