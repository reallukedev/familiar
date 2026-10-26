import { lazy, Suspense, useState } from 'react';
import { ApprovalList } from '../../components/Approvals';
import { ActivityRow } from '../../components/ActivityRow';
import { Banner, CountBadge, Pill, StatusLabel } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Icon } from '../../components/Icon';
import { CellText, GlyphTile, Row, Section } from '../../components/List';
import { Familiar } from '../../components/Familiar';
import { MacScreen } from '../../components/MacScreen';
import { SkeletonRows } from '../../components/Skeleton';
import { Switch } from '../../components/Switch';
import type { LiveState } from '../../components/LiveScreen';
import { api } from '../../lib/api';
import { keys, useEntry, useQuery } from '../../lib/cache';
import { bytes, count, memory, uptime, whenInline } from '../../lib/format';
import { usePlatform } from '../../lib/platform';
import { navigate, onLinkClick } from '../../lib/router';
import { macPath } from '../../lib/routes';
import { GUEST_PERMISSIONS, hasScreen, instanceInfo, missingPermissions, stateDetail } from '../../lib/status';
import type { ActivityPage, Approval } from '../../lib/types';
import { Page } from '../../shells/Page';
import { MAC_NAV, sectionBadge } from '../../shells/nav';
import type { MacPageProps } from './MacRoute';
import { PowerMenu } from './PowerMenu';
import { usePause, usePower } from './usePower';
import styles from './Overview.module.css';

const LiveScreen = lazy(() => import('../../components/LiveScreen'));

export function Overview({ instance }: MacPageProps) {
  const phone = usePlatform() === 'phone';
  const power = usePower(instance);
  const pause = usePause(instance);
  const info = instanceInfo(instance);
  const approvals = useQuery<Approval[]>(keys.approvals(instance.id), () => api.approvals(instance.id));
  const activity = useQuery<ActivityPage>(keys.activity(instance.id, 'all'), () => api.activity(instance.id, { limit: 50 }));
  const fresh = useEntry<string[]>(keys.fresh(instance.id)).data;
  const [live, setLive] = useState(false);
  const [liveState, setLiveState] = useState<LiveState>({ phase: 'connecting' });
  const canShow = hasScreen(instance.state) && instance.state !== 'booting' && instance.driver === 'qemu';
  const showingLive = live && canShow;
  const missing = missingPermissions(instance);
  const now = Date.now();

  const screenAction = canShow ? (
    showingLive ? (
      <span className={styles.liveControls}>
        {liveState.phase !== 'connected' ? <span className={styles.liveNote}>{liveState.phase === 'failed' ? 'Couldn’t connect' : 'Connecting…'}</span> : null}
        <Button variant="on-dark" size="small" onClick={() => setLive(false)}>
          Hide Screen
        </Button>
        <Button variant="on-dark" size="small" icon={<Icon name="expand" size={14} weight={2.2} />} onClick={() => navigate(macPath(instance.id, 'screen'))}>
          Open
        </Button>
      </span>
    ) : (
      <Button variant="on-dark" size="small" icon={<Icon name="eye" size={15} weight={2} />} onClick={() => setLive(true)}>
        Show Screen
      </Button>
    )
  ) : null;

  return (
    <Page title={instance.name} back={{ href: '/', label: 'Macs' }} width="wide" actions={<PowerMenu instance={instance} power={power} />}>
      <div className={styles.layout}>
        <div className={styles.display}>
          <MacScreen
            instance={instance}
            size="hero"
            onPower={power.run}
            powerBusy={!!power.busy}
            action={screenAction}
            live={
              showingLive ? (
                <Suspense fallback={null}>
                  <LiveScreen instanceId={instance.id} viewOnly onState={setLiveState} label={`${instance.name} live screen, view only`} />
                </Suspense>
              ) : undefined
            }
          />
        </div>

        <div className={styles.side}>
          <div className={styles.status}>
            <StatusLabel tone={info.tone} pulse={info.transitional}>
              <span className="t-headline">{info.label}</span>
            </StatusLabel>
            {instance.state === 'ready' ? null : <p className="t-subhead secondary pretty">{stateDetail(instance)}</p>}
            {/* Off, asleep or stopped: the display carries the button. */}
            {info.primary && !phone && instance.state === 'degraded' ? (
              <div className={styles.primary}>
                <Button variant="filled" onClick={() => power.run(info.primary!.action)} busy={!!power.busy}>
                  {info.primary.label}
                </Button>
              </div>
            ) : null}
            {instance.state === 'ready' && instance.last_active_at ? <p className="t-subhead secondary">Last used {whenInline(instance.last_active_at, now)}</p> : null}
          </div>

          <ApprovalList approvals={approvals.data ?? []} />

          {instance.counts.pending_review && !phone ? (
            <Section>
              <Row
                href={macPath(instance.id, 'review')}
                title={`${count(instance.counts.pending_review, 'change')} to review`}
                // Desktop only, so the macOS tile size.
                leading={<Familiar state="attention" size={22} />}
              />
            </Section>
          ) : null}

          <Section footer="Requests are refused while paused.">
            <Row
              title="Pause assistant"
              subtitle={pause.paused ? 'Paused' : undefined}
              leading={
                <GlyphTile color={pause.paused ? 'orange' : 'gray'}>
                  <Icon name="pause" size={16} weight={2.6} />
                </GlyphTile>
              }
              trailing={<Switch checked={pause.paused} onCheckedChange={pause.set} label="Pause assistant" tone="orange" />}
            />
          </Section>
        </div>
      </div>

      {missing.length ? (
        <div className={styles.banner}>
          <Banner tone="orange" icon="warning" title={`${count(missing.length, 'permission')} missing on the Mac`}>
            {missing.map((item) => (
              <p key={item.key}>
                {item.label}: {item.fix}
              </p>
            ))}
          </Banner>
        </div>
      ) : null}

      {phone ? (
        <Section>
          {MAC_NAV.slice(1).map((item) => (
            <Row
              key={item.section}
              href={macPath(instance.id, item.section)}
              title={item.label}
              leading={
                <GlyphTile color={item.section === 'permissions' ? 'orange' : item.section === 'screen' ? 'indigo' : item.section === 'connect' ? 'green' : item.section === 'activity' ? 'blue' : 'gray'}>
                  <Icon name={item.icon} size={17} weight={2} />
                </GlyphTile>
              }
              trailing={(() => {
                const badge = sectionBadge(item.section, instance.counts);
                return badge ? <CountBadge {...badge} /> : undefined;
              })()}
            />
          ))}
        </Section>
      ) : null}

      <div className={styles.columns}>
        <Section
          title="Recent activity"
          accessory={
            <a href={macPath(instance.id, 'activity')} onClick={(event) => onLinkClick(event)} className={`t-subhead ${styles.showAll}`}>
              Show All
            </a>
          }
        >
          {activity.data ? (
            activity.data.items.length ? (
              <ul>
                {activity.data.items.slice(0, 6).map((item) => (
                  <ActivityRow key={item.id} item={item} now={now} fresh={fresh?.includes(item.id)} />
                ))}
              </ul>
            ) : (
              <CellText>Nothing yet. When your assistant uses this Mac, each request shows up here with how it went.</CellText>
            )
          ) : activity.error ? (
            <CellText>{activity.error.message}</CellText>
          ) : (
            <SkeletonRows rows={3} />
          )}
        </Section>

        <AboutMac instance={instance} />
      </div>
    </Page>
  );
}

function AboutMac({ instance }: MacPageProps) {
  const guest = instance.guest;
  const display = guest?.displays.find((item) => item.main) ?? guest?.displays[0];
  return (
    <div>
      <Section title="About this Mac" footer={guest ? undefined : 'Appears once the Familiar agent connects.'}>
        {guest ? <Row title="macOS" value={`${guest.os_version} (${guest.os_build})`} /> : null}
        {instance.driver === 'qemu' ? null : <Row title="Runs" value={instance.remote_host ?? 'On your network'} />}
        {instance.driver === 'qemu' ? <Row title="Cores and memory" value={`${instance.cpus} cores, ${memory(instance.memory_mb)}`} /> : null}
        {guest ? <Row title="Free disk space" value={bytes(guest.disk_free_bytes)} /> : null}
        {display ? <Row title="Display" value={`${display.width} × ${display.height}${display.scale > 1 ? ' Retina' : ''}`} /> : null}
        {guest && instance.booted_at && instance.state === 'ready' ? <Row title="Up for" value={uptime(guest.uptime_s)} /> : null}
        {instance.agent?.version ? (
          <Row
            title="Familiar agent"
            value={instance.agent.version}
            trailing={instance.agent.available ? <Pill tone="blue">{instance.agent.updating ? 'Updating' : 'Update available'}</Pill> : undefined}
            href={instance.agent.available ? `${macPath(instance.id, 'settings')}#agent` : undefined}
            label={instance.agent.available ? `Familiar agent ${instance.agent.version}, update available. Open Settings.` : undefined}
          />
        ) : null}
      </Section>
      {guest ? (
        <Section title="What the assistant needs" footer="If one is off, its tools stop working.">
          {GUEST_PERMISSIONS.map((item) => {
            const state = guest.permissions[item.key];
            return (
              <Row
                key={item.key}
                align="top"
                title={item.label}
                subtitle={state === false ? item.fix : item.need}
                trailing={
                  state === true ? (
                    <StatusLabel tone="green">On</StatusLabel>
                  ) : state === false ? (
                    <StatusLabel tone="orange">Off</StatusLabel>
                  ) : (
                    <StatusLabel tone="gray">Unknown</StatusLabel>
                  )
                }
              />
            );
          })}
        </Section>
      ) : null}
    </div>
  );
}

