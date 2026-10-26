import { useEffect, useOptimistic, useState, useTransition } from 'react';
import { EmptyState } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Icon, type IconName } from '../../components/Icon';
import { CellText, GlyphTile, Row, Section } from '../../components/List';
import { Sheet } from '../../components/Sheet';
import { Picker } from '../../components/Picker';
import { SkeletonRows } from '../../components/Skeleton';
import { ApiError, api } from '../../lib/api';
import { fetchKey, keys, setData, updateData, useQuery } from '../../lib/cache';
import { sendLatest } from '../../lib/latest';
import { scrollToHash } from '../../lib/router';
import { macPath } from '../../lib/routes';
import { notify } from '../../lib/toast';
import type { ApprovalMode, Instance, PermissionGroup, PermissionMode } from '../../lib/types';
import { Page } from '../../shells/Page';
import { AppTile, RiskPill } from './Apps';
import type { MacPageProps } from './MacRoute';
import styles from './Permissions.module.css';

const MODES: { value: PermissionMode; label: string; hint: string }[] = [
  { value: 'allow', label: 'Allow', hint: 'Runs without asking' },
  { value: 'ask', label: 'Ask', hint: 'Waits for you to allow it' },
  { value: 'deny', label: 'Never', hint: 'Always refused' },
];
const MODE_LABEL: Record<PermissionMode, string> = { allow: 'Allow', ask: 'Ask', deny: 'Never' };
// An app server is off until it's turned on, so its third choice reads as Off.
const SERVER_MODES = MODES.map((mode) => (mode.value === 'deny' ? { ...mode, label: 'Off' } : mode));
const isServer = (group: PermissionGroup) => group.id.startsWith('mcp:');

export function PermissionsPage({ instance }: MacPageProps) {
  const groups = useQuery<PermissionGroup[]>(keys.permissions(instance.id), () => api.permissions(instance.id));

  useEffect(() => {
    if (groups.data) scrollToHash('center');
  }, [groups.data]);

  const builtIn = groups.data?.filter((group) => !group.id.startsWith('pack:') && !isServer(group)) ?? [];
  const packs = groups.data?.filter((group) => group.id.startsWith('pack:')) ?? [];
  const servers = groups.data?.filter(isServer) ?? [];

  return (
    <Page title="Permissions" back={{ href: macPath(instance.id), label: instance.name }}>
      {groups.error && !groups.data ? (
        <EmptyState icon="wifi" title="Couldn’t load permissions" action={<Button variant="tinted" onClick={() => void groups.reload()}>Try Again</Button>}>
          {groups.error.message}
        </EmptyState>
      ) : !groups.data ? (
        <div className={styles.loading}>
          <SkeletonRows rows={5} label="Loading permissions" />
        </div>
      ) : (
        <>
          <ApprovalModeSection instanceId={instance.id} />
          <Section title="On the Mac">
            {builtIn.map((group) => (
              <GroupCell key={group.id} instanceId={instance.id} group={group} />
            ))}
          </Section>
          {packs.length ? (
            <Section title="App tools" footer="Tools that send messages or email also follow Outgoing.">
              {packs.map((group) => (
                <GroupCell key={group.id} instanceId={instance.id} group={group} />
              ))}
            </Section>
          ) : null}
          <AppServersSection instance={instance} servers={servers} />
        </>
      )}
    </Page>
  );
}

// Bumped by every permission change, so a slow look for app servers can't show modes
// from before a change made while it ran.
let permissionWrites = 0;

type Search = { state: 'unchecked' } | { state: 'looking' } | { state: 'done' } | { state: 'failed'; message: string };

/**
 * MCP servers that apps on the Mac provide. Looking starts none of them; each stays off
 * until the owner picks Ask or Allow, and only then are its tools listed.
 */
function AppServersSection({ instance, servers }: { instance: Instance; servers: PermissionGroup[] }) {
  const [search, setSearch] = useState<Search>({ state: 'unchecked' });
  const running = instance.state === 'ready' || instance.state === 'degraded';

  async function look() {
    setSearch({ state: 'looking' });
    const writes = permissionWrites;
    try {
      const groups = await api.findAppServers(instance.id);
      if (writes === permissionWrites) setData(keys.permissions(instance.id), groups);
      else await fetchKey(keys.permissions(instance.id), () => api.permissions(instance.id));
      setSearch({ state: 'done' });
    } catch (error) {
      setSearch({ state: 'failed', message: error instanceof ApiError ? error.message : 'Couldn’t ask the Mac. Check your connection and try again.' });
    }
  }

  // Look once when the page opens on a running Mac, so new apps show up on their own.
  const unchecked = search.state === 'unchecked';
  useEffect(() => {
    if (running && unchecked) void look();
  }, [running, unchecked]);

  const looking = search.state === 'looking';
  return (
    <Section
      title="App servers"
      live="polite"
      accessory={
        <Button variant="plain" size="small" busy={looking} disabled={!running} onClick={() => void look()}>
          Look Again
        </Button>
      }
      footer="MCP servers that apps on this Mac provide. Each stays off until you choose Ask or Allow, and Familiar doesn’t start one before then. Approvals above can make one ask first, but never turn one on."
    >
      {servers.length ? (
        servers.map((group) => <GroupCell key={group.id} instanceId={instance.id} group={group} />)
      ) : search.state === 'failed' ? (
        <CellText>{search.message}</CellText>
      ) : !running ? (
        <CellText>Start the Mac to look for app servers.</CellText>
      ) : looking || unchecked ? (
        <SkeletonRows rows={1} label="Looking for app servers" />
      ) : (
        <CellText>No MCP servers found on this Mac yet. Apps like Xcode and Figma, and servers set up in Claude, Cursor or VS Code, appear here.</CellText>
      )}
      {servers.length && search.state === 'failed' ? <CellText>{search.message}</CellText> : null}
    </Section>
  );
}

type ChosenMode = Exclude<ApprovalMode, 'custom'>;
const APPROVAL_MODES: { value: ApprovalMode; label: string; footer: string }[] = [
  { value: 'ask', label: 'Ask for Everything', footer: 'Your assistant waits for you before everything it does, even looking at the screen.' },
  { value: 'changes', label: 'Ask Before Changes', footer: 'Your assistant can look and read on its own, and waits for you before it changes or sends anything.' },
  { value: 'all', label: 'Allow Everything', footer: 'Your assistant works without waiting. Look over what it changed in Review.' },
  { value: 'custom', label: 'Custom', footer: 'You’ve chosen each permission below.' },
];

/** One choice that sets every permission below. Custom appears only when they match no mode. */
function ApprovalModeSection({ instanceId }: { instanceId: string }) {
  const query = useQuery<{ mode: ApprovalMode }>(keys.approvalMode(instanceId), () => api.approvalMode(instanceId));
  const [mode, setMode] = useOptimistic(query.data?.mode ?? null);
  const [, startTransition] = useTransition();
  const change = (next: ApprovalMode) => {
    if (next === 'custom' || next === mode) return;
    startTransition(async () => {
      setMode(next);
      try {
        const updated = await sendLatest(`${instanceId}:approval-mode`, next as ChosenMode, (chosen) => api.setApprovalMode(instanceId, chosen));
        if (!updated) return;
        setData(keys.permissions(instanceId), updated.groups);
        setData(keys.approvalMode(instanceId), { mode: updated.mode });
      } catch (error) {
        notify.error(error);
      }
    });
  };
  const options = APPROVAL_MODES.filter((option) => option.value !== 'custom' || mode === 'custom');
  const current = APPROVAL_MODES.find((option) => option.value === mode);
  return (
    <Section title="Approvals" footer={current?.footer ?? '\u00a0'}>
      <Row
        leading={
          <GlyphTile color="orange">
            <Icon name="hand" size={16} weight={2} />
          </GlyphTile>
        }
        title="Assistant waits for you"
        trailing={
          <span className={styles.end}>
            <span className={`${styles.menu} ${styles.modeMenu}`}>
              {mode ? (
                <Picker label="When your assistant waits for you" value={mode} onChange={change} options={options.map(({ value, label }) => ({ value, label }))} />
              ) : null}
            </span>
          </span>
        }
      />
    </Section>
  );
}

const GROUP_ICON: Record<string, { icon: IconName; color: 'blue' | 'indigo' | 'green' | 'orange' | 'red' | 'gray' }> = {
  observe: { icon: 'eye', color: 'blue' },
  control: { icon: 'cursor', color: 'indigo' },
  files_read: { icon: 'folder', color: 'blue' },
  files_write: { icon: 'doc', color: 'orange' },
  shell: { icon: 'terminal', color: 'gray' },
  clipboard: { icon: 'clipboard', color: 'gray' },
  apps_manage: { icon: 'grid', color: 'indigo' },
  snapshots: { icon: 'stack', color: 'green' },
  outgoing: { icon: 'envelope', color: 'red' },
};

/**
 * One permission as macOS Privacy & Security shows it: icon, name, a short
 * description, and a pop-up menu. ⓘ lists the tools it covers.
 */
function GroupCell({ instanceId, group }: { instanceId: string; group: PermissionGroup }) {
  const [mode, setMode] = useOptimistic(group.mode);
  const [, startTransition] = useTransition();
  const [info, setInfo] = useState(false);
  const change = (next: PermissionMode) =>
    startTransition(async () => {
      setMode(next);
      permissionWrites += 1;
      try {
        // One at a time per group, newest choice last, so a slow reply can't undo a later pick.
        const updated = await sendLatest(`${instanceId}:${group.id}`, next, (mode) => api.setPermission(instanceId, group.id, mode));
        if (updated) updateData<PermissionGroup[]>(keys.permissions(instanceId), (list) => list.map((item) => (item.id === updated.id ? updated : item)));
        // A group set by hand may no longer match the approval mode.
        if (updated) void fetchKey(keys.approvalMode(instanceId), () => api.approvalMode(instanceId)).catch(() => {});
      } catch (error) {
        notify.error(error);
      }
    });
  const changed = mode !== group.default_mode;
  const look = GROUP_ICON[group.id] ?? { icon: 'grid' as IconName, color: 'gray' as const };
  const server = group.server;
  const options = (server ? SERVER_MODES : MODES).map(({ value, label }) => ({ value, label }));
  const tools = group.tools.length;
  const subtitle = server
    ? server.changed && mode === 'deny'
      ? 'Its command or settings changed since you turned it on. Turn it on again to allow it as it is now.'
      : mode !== 'deny' && server.tools_error
      ? server.tools_error
      : mode !== 'deny' && tools
        ? `${group.description} ${tools === 1 ? 'One tool' : `${tools} tools`}.`
        : group.description
    : changed
      ? `${group.description} Default is ${MODE_LABEL[group.default_mode]}.`
      : group.description;

  return (
    <div id={`group-${group.id}`} className={styles.anchor}>
      <Row
        leading={
          group.id.startsWith('pack:') || server?.app ? (
            <AppTile app={{ name: server?.app ?? group.label, icon: null }} />
          ) : server ? (
            <GlyphTile color="gray">
              <Icon name="server" size={16} weight={2} />
            </GlyphTile>
          ) : (
            <GlyphTile color={look.color}>
              <Icon name={look.icon} size={16} weight={2} />
            </GlyphTile>
          )
        }
        title={group.label}
        subtitle={subtitle}
        trailing={
          <span className={styles.end}>
            <span className={styles.menu}>
              <Picker label={`${group.label} permission`} value={mode} onChange={change} options={options} />
            </span>
            {group.tools.length || server ? (
              <button type="button" className={styles.info} onClick={() => setInfo(true)} aria-label={`Tools in ${group.label}`}>
                <Icon name="info" size={17} weight={1.8} />
              </button>
            ) : null}
          </span>
        }
      />
      <Sheet open={info} onOpenChange={setInfo} title={group.label} description={group.description} dismissLabel="Done">
        {server ? (
          <Section title={server.transport === 'http' ? 'Address' : 'Command'} footer={server.source === 'app' ? `Comes with ${server.app ?? group.label}.` : `Found in ${server.source}’s MCP settings on the Mac. Settings like API keys stay on the Mac.`}>
            <CellText>
              <code className={styles.detail}>{server.detail}</code>
            </CellText>
          </Section>
        ) : null}
        {group.tools.length ? (
          <Section title={server ? 'Tools' : undefined} footer={changed || server ? undefined : `${MODE_LABEL[group.default_mode]} is the default.`}>
            {group.tools.map((tool) => (
              <Row key={tool.name} title={tool.title} subtitle={<code>{tool.name}</code>} trailing={<RiskPill risk={tool.risk} />} />
            ))}
          </Section>
        ) : server ? (
          <Section title="Tools">
            <CellText>{mode === 'deny' ? 'Choose Ask or Allow to see its tools. Familiar starts it only then.' : (server.tools_error ?? 'It hasn’t listed any tools.')}</CellText>
          </Section>
        ) : null}
        {changed ? (
          <Section>
            <Row title={server ? 'Turn Off' : `Use Default (${MODE_LABEL[group.default_mode]})`} tone="tint" onClick={() => { change(group.default_mode); setInfo(false); }} chevron={false} />
          </Section>
        ) : null}
      </Sheet>
    </div>
  );
}
