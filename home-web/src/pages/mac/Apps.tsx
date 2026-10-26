import { useDeferredValue, useMemo, useOptimistic, useState, useTransition, type CSSProperties, type FormEvent } from 'react';
import { Banner, EmptyState, Pill } from '../../components/Bits';
import { Button, IconButton } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { Fields, FormError, TextArea, TextField } from '../../components/Field';
import { Icon } from '../../components/Icon';
import { CellText, Row, Section } from '../../components/List';
import { ActionMenu } from '../../components/Menu';
import { Segmented } from '../../components/Segmented';
import { Sheet } from '../../components/Sheet';
import { SkeletonRows } from '../../components/Skeleton';
import { Switch } from '../../components/Switch';
import { ApiError, api } from '../../lib/api';
import { fetchKey, keys, updateData, useQuery } from '../../lib/cache';
import { count } from '../../lib/format';
import { PACK_TEMPLATE, lineForField, lintManifest } from '../../lib/pack';
import { usePlatform } from '../../lib/platform';
import { macPath } from '../../lib/routes';
import { isOn } from '../../lib/status';
import { notify } from '../../lib/toast';
import type { AppEntry, InstallSource, ToolRisk } from '../../lib/types';
import { appStoreId, appStoreIdError, caskError, downloadUrlError, sha256Error } from '../../lib/validate';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Apps.module.css';
import { withUndo } from '../../lib/undo';

const KIND_ORDER: { kind: AppEntry['kind']; title: string; footer?: string }[] = [
  { kind: 'builtin', title: 'Apple apps' },
  { kind: 'custom', title: 'Your tools' },
  { kind: 'scriptable', title: 'Scriptable apps', footer: 'Apps your assistant can script with AppleScript.' },
  { kind: 'ui_only', title: 'Other apps', footer: 'Used by looking at the screen. No setup needed.' },
];

function upsertApp(instanceId: string, app: AppEntry) {
  updateData<AppEntry[]>(keys.apps(instanceId), (list) => {
    const index = list.findIndex((item) => item.id === app.id);
    if (index === -1) return [...list, app];
    const next = list.slice();
    next[index] = app;
    return next;
  });
}

export function AppsPage({ instance }: MacPageProps) {
  const phone = usePlatform() === 'phone';
  const apps = useQuery<AppEntry[]>(keys.apps(instance.id), () => api.apps(instance.id));
  const [query, setQuery] = useState('');
  const deferred = useDeferredValue(query.trim().toLowerCase());
  const [installing, setInstalling] = useState(false);
  const [adding, setAdding] = useState(false);
  const [detail, setDetail] = useState<AppEntry | null>(null);

  const filtered = useMemo(() => (apps.data ?? []).filter((app) => !deferred || `${app.name} ${app.bundle_id ?? ''} ${app.tools.map((tool) => tool.name).join(' ')}`.toLowerCase().includes(deferred)), [apps.data, deferred]);
  const running = isOn(instance.state) && instance.state !== 'sleeping';

  const actions = phone ? (
    <ActionMenu
      label="Add apps"
      trigger={
        <IconButton label="Add apps">
          <Icon name="plus" size={22} weight={2.2} />
        </IconButton>
      }
      items={[
        { label: 'Install App', onSelect: () => setInstalling(true) },
        { label: 'Add Custom Tools', onSelect: () => setAdding(true) },
      ]}
    />
  ) : (
    <>
      <Button variant="gray" onClick={() => setAdding(true)}>
        Add Custom Tools
      </Button>
      <Button variant="filled" onClick={() => setInstalling(true)}>
        Install App
      </Button>
    </>
  );

  return (
    <Page title="Apps" back={{ href: macPath(instance.id), label: instance.name }} actions={actions}>
      {!running && apps.data?.length ? (
        <div className={styles.note}>
          <Banner tone="gray" icon="info" title="Showing the last list Familiar saw">
            Start the Mac to see what’s installed right now.
          </Banner>
        </div>
      ) : null}

      {apps.data && apps.data.length > 8 ? (
        <div className={styles.search}>
          <TextField label="Search apps and tools" labelHidden value={query} onValueChange={setQuery} placeholder="Search apps and tools…" type="search" autoComplete="off" />
        </div>
      ) : null}

      {apps.error && !apps.data ? (
        <EmptyState icon="wifi" title="Couldn’t load apps" action={<Button variant="tinted" onClick={() => void apps.reload()}>Try Again</Button>}>
          {apps.error.message}
        </EmptyState>
      ) : !apps.data ? (
        <div className={styles.note}>
          <SkeletonRows rows={6} leading label="Loading apps" />
        </div>
      ) : apps.data.length === 0 ? (
        <EmptyState icon="grid" title={instance.guest ? 'No apps found' : 'Apps show up after the first start'} action={<Button variant="tinted" onClick={() => setInstalling(true)}>Install App</Button>}>
          {instance.guest ? (instance.driver === 'remote' ? 'Install an app with Homebrew, from a download link or from the App Store.' : 'Install an app from a download link, the App Store or Homebrew.') : 'Once the Mac has started and the Familiar agent inside it connects, every installed app is listed here.'}
        </EmptyState>
      ) : (
        KIND_ORDER.map(({ kind, title, footer }) => {
          const list = filtered.filter((app) => app.kind === kind);
          if (!list.length) return null;
          return (
            <Section key={kind} title={title} footer={footer}>
              {list.map((app) => (
                <AppRow key={app.id} instanceId={instance.id} app={app} onOpen={() => setDetail(app)} />
              ))}
            </Section>
          );
        })
      )}
      {apps.data?.length && !filtered.length ? <CellText>No apps or tools match “{query}”.</CellText> : null}

      <InstallSheet instanceId={instance.id} brewFirst={instance.driver === 'remote'} open={installing} onOpenChange={setInstalling} />
      <CustomToolsSheet instanceId={instance.id} open={adding} onOpenChange={setAdding} />
      <AppDetailSheet instanceId={instance.id} app={detail} onClose={() => setDetail(null)} />
    </Page>
  );
}

/** A stable hue per app, so a letter tile keeps its color between visits. */
function hueOf(name: string): number {
  // FNV-1a, then the golden angle, so similar names still land far apart on the wheel.
  let hash = 2166136261;
  for (const char of name) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return Math.round(((hash >>> 0) * 137.508) % 360);
}

export function AppTile({ app }: { app: Pick<AppEntry, 'name' | 'icon'> }) {
  return app.icon ? <img className={styles.icon} src={app.icon} alt="" width={30} height={30} loading="lazy" /> : <span className={`${styles.icon} ${styles.letter}`} style={{ '--hue': hueOf(app.name) } as CSSProperties} aria-hidden="true">{app.name.charAt(0)}</span>;
}

function AppRow({ instanceId, app, onOpen }: { instanceId: string; app: AppEntry; onOpen: () => void }) {
  const [enabled, setEnabled] = useOptimistic(app.enabled);
  const [, startTransition] = useTransition();
  const toggle = (value: boolean) =>
    startTransition(async () => {
      setEnabled(value);
      try {
        upsertApp(instanceId, await api.setApp(instanceId, app.id, value));
        notify.success(value ? `${app.name} tools on` : `${app.name} tools off`);
      } catch (error) {
        notify.error(error);
      }
    });

  const parts = [app.tools.length ? count(app.tools.length, 'tool') : null, app.installed ? null : 'Not installed'].filter(Boolean);
  const switchable = app.kind !== 'ui_only';
  return (
    <div className={styles.appRow}>
      <Row
        onClick={onOpen}
        title={app.name}
        subtitle={parts.join(' · ') || undefined}
        leading={<AppTile app={app} />}
        chevron={false}
        label={`${app.name} details`}
      />
      {switchable ? (
        <span className={styles.switch}>
          <Switch checked={enabled} onCheckedChange={toggle} label={`Enable ${app.name} for the assistant`} disabled={!app.installed} />
        </span>
      ) : null}
    </div>
  );
}

const RISK: Record<ToolRisk, { label: string; tone: 'gray' | 'blue' | 'orange' | 'red' }> = {
  read: { label: 'Reads', tone: 'gray' },
  write: { label: 'Changes', tone: 'blue' },
  send: { label: 'Sends', tone: 'orange' },
  system: { label: 'System', tone: 'red' },
};

export function RiskPill({ risk }: { risk: ToolRisk }) {
  return <Pill tone={RISK[risk].tone}>{RISK[risk].label}</Pill>;
}

function AppDetailSheet({ instanceId, app, onClose }: { instanceId: string; app: AppEntry | null; onClose: () => void }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  async function remove() {
    if (!app) return;
    setBusy(true);
    try {
      await api.removeCustomPack(instanceId, app.id);
      updateData<AppEntry[]>(keys.apps(instanceId), (list) => list.filter((item) => item.id !== app.id));
      notify.success(`Removed ${app.name} tools`);
      onClose();
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Sheet open={!!app} onOpenChange={(open) => !open && onClose()} title={app?.name ?? ''} description={app?.bundle_id ?? undefined}>
      {app ? (
        <div className={styles.detail}>
          {app.tools.length ? (
            <Section title={count(app.tools.length, 'tool')}>
              {app.tools.map((tool) => (
                <Row key={tool.name} title={tool.title} subtitle={<code>{tool.name}</code>} trailing={<RiskPill risk={tool.risk} />} />
              ))}
            </Section>
          ) : (
            <CellText>{app.kind === 'ui_only' ? 'Your assistant uses this app through the screen, with clicks and typing.' : 'This app has no tools of its own.'}</CellText>
          )}
          {app.kind === 'custom' ? (
            <div className={styles.remove}>
              <Button variant="destructive" width="full" onClick={() => setConfirming(true)} busy={busy}>
                Remove Tools
              </Button>
            </div>
          ) : null}
          <Confirm open={confirming} onOpenChange={setConfirming} title={`Remove ${app.name} tools?`} message="Your assistant loses these tools right away. The app itself stays installed." action="Remove Tools" onConfirm={() => withUndo(`Removing ${app.name} tools…`, remove)} />
        </div>
      ) : null}
    </Sheet>
  );
}

type Source = 'url' | 'app_store' | 'brew';

// Macs on this server are Intel (all OSX-KVM can emulate), which Homebrew no longer
// supports, so they default to a download; remote Macs are usually Apple silicon.
function InstallSheet({ instanceId, brewFirst, open, onOpenChange }: { instanceId: string; brewFirst: boolean; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [source, setSource] = useState<Source>(brewFirst ? 'brew' : 'url');
  const [url, setUrl] = useState('');
  const [sha, setSha] = useState('');
  const [store, setStore] = useState('');
  const [cask, setCask] = useState('');
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const errors = source === 'url' ? { url: downloadUrlError(url.trim()), sha: sha256Error(sha) } : source === 'app_store' ? { store: appStoreIdError(appStoreId(store)) } : { cask: caskError(cask) };

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitted(true);
    if (Object.values(errors).some(Boolean)) return;
    const body: InstallSource = source === 'url' ? { type: 'url', url: url.trim(), ...(sha.trim() ? { sha256: sha.trim().toLowerCase() } : {}) } : source === 'app_store' ? { type: 'app_store', id: appStoreId(store) } : { type: 'brew', cask: cask.trim() };
    setBusy(true);
    setError(null);
    try {
      const result = await api.installApp(instanceId, body);
      if ('installing' in result) {
        // Still going after the request's time limit: check back in a minute.
        notify.info('Still installing. The app appears here when it’s done.');
        setTimeout(() => void fetchKey(keys.apps(instanceId), () => api.apps(instanceId)).catch(() => {}), 60_000);
      } else {
        upsertApp(instanceId, result);
        notify.success(`Installed ${result.name}`);
      }
      onOpenChange(false);
      setUrl('');
      setSha('');
      setStore('');
      setCask('');
      setSubmitted(false);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : 'Couldn’t start the install. Try again.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Install App" primary={{ label: 'Install', form: 'install-app', busy }}>
      <form id="install-app" className={styles.form} onSubmit={submit} noValidate>
        <Segmented
          label="Install from"
          value={source}
          onChange={(value) => {
            setSource(value);
            setSubmitted(false);
            setError(null);
          }}
          options={
            brewFirst
              ? [
                  { value: 'brew', label: 'Homebrew' },
                  { value: 'url', label: 'Download' },
                  { value: 'app_store', label: 'App Store' },
                ]
              : [
                  { value: 'url', label: 'Download' },
                  { value: 'app_store', label: 'App Store' },
                  { value: 'brew', label: 'Homebrew' },
                ]
          }
        />
        <Fields>
          {source === 'url' ? (
            <>
              <TextField label="Download link" value={url} onValueChange={setUrl} placeholder="https://example.com/App.dmg" type="url" inputMode="url" autoCapitalize="none" spellCheck={false} error={submitted ? (errors as { url: string | null }).url : null} description="An https link to a .dmg, .zip or .pkg." />
              <TextField label="SHA-256 checksum (optional)" value={sha} onValueChange={setSha} mono autoCapitalize="none" spellCheck={false} error={submitted ? (errors as { sha: string | null }).sha : null} description="If the developer publishes one, Familiar checks the download against it before installing." />
            </>
          ) : source === 'app_store' ? (
            <TextField label="App Store link or ID" value={store} onValueChange={setStore} placeholder="https://apps.apple.com/app/id497799835" autoCapitalize="none" spellCheck={false} error={submitted ? (errors as { store: string | null }).store : null} description="The Mac must be signed in to the App Store, and the app must already be in that account’s purchases." />
          ) : (
            <TextField label="Homebrew cask" value={cask} onValueChange={setCask} placeholder="visual-studio-code" mono autoCapitalize="none" spellCheck={false} error={submitted ? (errors as { cask: string | null }).cask : null} description="The name from brew.sh, for example firefox or visual-studio-code." />
          )}
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}

function CustomToolsSheet({ instanceId, open, onOpenChange }: { instanceId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [manifest, setManifest] = useState(PACK_TEMPLATE);
  const deferred = useDeferredValue(manifest);
  const issues = useMemo(() => lintManifest(deferred), [deferred]);
  const [server, setServer] = useState<{ message: string; line: number | null } | null>(null);
  const [busy, setBusy] = useState(false);

  function goToLine(line: number | null) {
    const area = document.getElementById('pack-manifest') as HTMLTextAreaElement | null;
    if (!area || !line) return;
    const lines = manifest.split('\n');
    const start = lines.slice(0, line - 1).reduce((sum, text) => sum + text.length + 1, 0);
    area.focus();
    area.setSelectionRange(start, start + (lines[line - 1]?.length ?? 0));
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (issues.length) {
      goToLine(issues[0]?.line ?? null);
      return;
    }
    setBusy(true);
    setServer(null);
    try {
      const app = await api.addCustomPack(instanceId, manifest);
      upsertApp(instanceId, app);
      notify.success(`Added ${app.name} tools`);
      onOpenChange(false);
    } catch (cause) {
      if (cause instanceof ApiError) {
        const line = lineForField(manifest, cause.field);
        setServer({ message: cause.message, line });
        goToLine(line);
      } else setServer({ message: 'Couldn’t add these tools. Try again.', line: null });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Add Custom Tools" size="large" primary={{ label: 'Add Tools', form: 'custom-tools', busy }} description="Paste a pack manifest. Each tool runs a script in the Mac with its arguments passed as JSON, never pasted into the code.">
      <form id="custom-tools" className={styles.form} onSubmit={submit} noValidate>
        <TextArea id="pack-manifest" label="Pack manifest (TOML)" value={manifest} onValueChange={(value) => { setManifest(value); setServer(null); }} spellCheck={false} autoCapitalize="none" autoCorrect="off" rows={16} error={server ? (server.line ? `Line ${server.line}: ${server.message}` : server.message) : null} />
        {issues.length ? (
          <div className={styles.issues} role="status">
            <p className="t-footnote secondary">{count(issues.length, 'thing')} to fix before adding:</p>
            <ul>
              {issues.map((issue) => (
                <li key={`${issue.field}-${issue.message}`}>
                  <button type="button" className={styles.issue} onClick={() => goToLine(issue.line)} disabled={!issue.line}>
                    {issue.line ? <span className={`${styles.line} tabular`}>Line {issue.line}</span> : null}
                    <span>{issue.message}</span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p className="t-footnote text-green">The manifest looks right. Familiar checks it again when you add it.</p>
        )}
      </form>
    </Sheet>
  );
}
