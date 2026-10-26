import { Dialog } from '@base-ui/react/dialog';
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { notificationPermission, requestNotifications } from '../lib/alerts';
import { isMacKeyboard, usePlatform } from '../lib/platform';
import { setPreferences, usePreferences } from '../lib/preferences';
import { currentSubscription, pushSupported, subscribePush, unsubscribePush } from '../lib/push';
import { showNewRecoveryKey } from '../lib/recovery';
import { navigate } from '../lib/router';
import { useSession } from '../lib/session';
import { notify } from '../lib/toast';
import { closeSettings, openSettings, selectSettingsTab, useSettingsWindow, type SettingsTab } from '../lib/settingsWindow';
import type { Preferences } from '../lib/types';
import { Avatar } from './Bits';
import { Button } from './Button';
import { Icon, type IconName } from './Icon';
import { Row, Section } from './List';
import { Picker } from './Picker';
import { Segmented } from './Segmented';
import { Sheet } from './Sheet';
import { Switch } from './Switch';
import styles from './SettingsWindow.module.css';

const TABS: { id: SettingsTab; label: string; icon: IconName }[] = [
  { id: 'general', label: 'General', icon: 'gear' },
  { id: 'screen', label: 'Screen', icon: 'display' },
  { id: 'notifications', label: 'Notifications', icon: 'bell' },
  { id: 'shortcuts', label: 'Shortcuts', icon: 'keyboard' },
];

const QUALITY: { value: Preferences['screen_quality']; label: string; hint: string }[] = [
  { value: 'auto', label: 'Automatic', hint: 'Adjusts to your connection.' },
  { value: 'sharp', label: 'Sharp', hint: 'Uses the most bandwidth.' },
  { value: 'balanced', label: 'Balanced', hint: 'Good for Wi‑Fi.' },
  { value: 'fast', label: 'Fast', hint: 'Lowest delay. Images look softer.' },
];

/**
 * Settings for the portal itself, saved to the account so they follow the
 * person to every device. A macOS Settings window on a Mac (⌘,) and an iOS
 * sheet on a phone. Also applies the chosen appearance to the page.
 */
export function SettingsWindow() {
  const platform = usePlatform();
  const { open, tab } = useSettingsWindow();
  const { appearance } = usePreferences();

  useEffect(() => {
    const root = document.documentElement;
    if (appearance === 'system') delete root.dataset.theme;
    else root.dataset.theme = appearance;
  }, [appearance]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        event.preventDefault();
        openSettings();
      }
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, []);

  const onOpenChange = (next: boolean) => (next ? openSettings() : closeSettings());

  if (platform === 'phone') {
    return (
      <Sheet open={open} onOpenChange={onOpenChange} title="Settings" dismissLabel="Done" size="large">
        <div className={styles.phoneBody}>
          <GeneralPane account={false} />
          <ScreenPane />
          <NotificationsPane />
        </div>
      </Sheet>
    );
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className={styles.backdrop} />
        <Dialog.Viewport className={styles.viewport}>
          <Dialog.Popup className={styles.window}>
            <header className={styles.titlebar}>
              <Dialog.Close className={styles.close} aria-label="Close Settings">
                <Icon name="xmark" size={8} weight={3} />
              </Dialog.Close>
              <Dialog.Title className={styles.title}>{TABS.find((item) => item.id === tab)?.label ?? 'Settings'}</Dialog.Title>
            </header>
            <nav className={styles.tabs} aria-label="Settings sections">
              {TABS.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={`${styles.tab} ${item.id === tab ? styles.tabSelected : ''}`}
                  aria-current={item.id === tab ? 'page' : undefined}
                  onClick={() => selectSettingsTab(item.id)}
                >
                  <Icon name={item.icon} size={20} weight={1.7} />
                  <span>{item.label}</span>
                </button>
              ))}
            </nav>
            <AutoHeight>
              <div className={styles.pane} key={tab}>
                {tab === 'general' ? <GeneralPane /> : tab === 'screen' ? <ScreenPane /> : tab === 'notifications' ? <NotificationsPane /> : <ShortcutsPane />}
              </div>
            </AutoHeight>
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Resizes the window to each pane like macOS Settings does, instead of jumping. */
function AutoHeight({ children }: { children: ReactNode }) {
  const inner = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    const node = inner.current;
    if (!node) return;
    const observer = new ResizeObserver(() => setHeight(node.offsetHeight));
    observer.observe(node);
    setHeight(node.offsetHeight);
    return () => observer.disconnect();
  }, []);
  return (
    <div className={styles.autoHeight} style={height === null ? undefined : { height }}>
      <div ref={inner}>{children}</div>
    </div>
  );
}

function GeneralPane({ account = true }: { account?: boolean }) {
  const prefs = usePreferences();
  const { data: session } = useSession();
  const user = session?.user;
  const newRecoveryKey = async () => {
    try {
      await showNewRecoveryKey();
    } catch (error) {
      notify.error(error);
    }
  };
  return (
    <>
      <Section title="Appearance">
        <div className={styles.controlRow}>
          <Segmented
            label="Appearance"
            value={prefs.appearance}
            onChange={(appearance) => void setPreferences({ appearance })}
            options={[
              { value: 'system', label: 'System' },
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
          />
        </div>
      </Section>
      <Section title="Undo" footer={prefs.undo_seconds ? 'Deleting, shutting down and similar actions wait this long, with an Undo button, before they happen.' : 'Actions happen as soon as you confirm them.'}>
        <Row
          title="Undo time"
          trailing={
            <Picker
              label="Undo time"
              value={String(prefs.undo_seconds) as '0' | '3' | '5' | '10'}
              onChange={(value) => void setPreferences({ undo_seconds: Number(value) as Preferences['undo_seconds'] })}
              options={[
                { value: '0', label: 'Off' },
                { value: '3', label: '3 Seconds' },
                { value: '5', label: '5 Seconds' },
                { value: '10', label: '10 Seconds' },
              ]}
            />
          }
        />
      </Section>
      <Section title="When Familiar Opens">
        <Row
          title="Show"
          trailing={
            <Picker
              label="When Familiar opens, show"
              value={prefs.start_page}
              onChange={(start_page) => void setPreferences({ start_page })}
              options={[
                { value: 'home', label: 'Home' },
                { value: 'last_mac', label: 'The Last Mac I Used' },
              ]}
            />
          }
        />
      </Section>
      {user && account ? (
        <Section title="Account" footer="Settings follow you to every browser.">
          <Row
            leading={<Avatar name={user.display_name} size="small" />}
            title={user.display_name}
            subtitle={`@${user.username}`}
            trailing={
              <Button
                variant="gray"
                onClick={() => {
                  closeSettings();
                  navigate('/account');
                }}
              >
                Account and Security
              </Button>
            }
          />
          <Row
            title="Recovery Key"
            subtitle={session?.recovery_key_needed ? 'Not saved yet' : 'Unlocks your Macs’ data if you forget your password.'}
            trailing={
              <Button variant="gray" onClick={() => void newRecoveryKey()}>
                Recovery Key…
              </Button>
            }
          />
        </Section>
      ) : null}
    </>
  );
}

function ScreenPane() {
  const prefs = usePreferences();
  const quality = QUALITY.find((item) => item.value === prefs.screen_quality) ?? QUALITY[0]!;
  return (
    <>
      <Section title="Opening a Mac’s Screen">
        <Row
          title="Start in"
          trailing={
            <Segmented
              label="Start in"
              size="compact"
              value={prefs.screen_mode}
              onChange={(screen_mode) => void setPreferences({ screen_mode })}
              options={[
                { value: 'view', label: 'View Only' },
                { value: 'control', label: 'Control' },
              ]}
            />
          }
        />
        <Row
          title="Size"
          trailing={
            <Segmented
              label="Screen size"
              size="compact"
              value={prefs.screen_scale}
              onChange={(screen_scale) => void setPreferences({ screen_scale })}
              options={[
                { value: 'fit', label: 'Fit to Window' },
                { value: 'actual', label: 'Actual Size' },
              ]}
            />
          }
        />
      </Section>
      <Section title="Quality" footer={quality.hint}>
        <Row
          title="Picture quality"
          trailing={
            <Picker label="Picture quality" value={prefs.screen_quality} onChange={(screen_quality) => void setPreferences({ screen_quality })} options={QUALITY.map(({ value, label }) => ({ value, label }))} />
          }
        />
      </Section>
      <Section
        title="Keyboard and Clipboard"
        footer={isMacKeyboard ? 'Works while you’re in control.' : 'Ctrl shortcuts reach the Mac as ⌘ shortcuts.'}
      >
        <Row
          title="Share clipboard with the Mac"
          subtitle="Text only"
          trailing={<Switch label="Share clipboard with the Mac" checked={prefs.clipboard_sync} onCheckedChange={(clipboard_sync) => void setPreferences({ clipboard_sync })} />}
        />
        {isMacKeyboard ? null : (
          <Row title="Use Ctrl as ⌘" trailing={<Switch label="Use Ctrl as Command" checked={prefs.map_ctrl_to_cmd} onCheckedChange={(map_ctrl_to_cmd) => void setPreferences({ map_ctrl_to_cmd })} />} />
        )}
      </Section>
    </>
  );
}

function NotificationsPane() {
  const prefs = usePreferences();
  const [permission, setPermission] = useState(notificationPermission);
  // Whether this browser holds a push subscription; null until checked.
  const [subscribed, setSubscribed] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const supported = pushSupported();

  useEffect(() => {
    let live = true;
    void currentSubscription().then((subscription) => {
      if (live) setSubscribed(subscription !== null);
    });
    return () => {
      live = false;
    };
  }, []);

  const toggle = async (next: boolean) => {
    setBusy(true);
    try {
      if (next) {
        // Ask first, straight from the click: Safari only shows the prompt for a gesture.
        const result = permission === 'granted' ? permission : await requestNotifications();
        setPermission(result);
        if (result !== 'granted') return;
        await subscribePush();
        setSubscribed(true);
      } else {
        await unsubscribePush();
        setSubscribed(false);
      }
      await setPreferences({ approval_notifications: next });
    } catch (error) {
      notify.error(error, next ? 'Couldn’t turn on notifications. Try again.' : 'Couldn’t turn off notifications. Try again.');
      setSubscribed((await currentSubscription()) !== null);
    } finally {
      setBusy(false);
    }
  };

  const blocked = permission === 'denied';
  const unsupported = permission === 'unsupported' || !supported;
  const footer = unsupported
    ? 'On iPhone, add Familiar to your Home Screen first.'
    : blocked
      ? 'Blocked in your browser’s site settings.'
      : 'Notifications arrive even when Familiar is closed.';
  const on = prefs.approval_notifications && permission === 'granted' && subscribed === true;

  return (
    <Section title="Approvals" footer={footer}>
      <Row
        title="Notify me when an assistant asks"
        subtitle={on ? 'On for this browser' : undefined}
        trailing={
          <Switch
            label="Notify me when an assistant asks for approval"
            checked={on}
            disabled={blocked || unsupported || busy || subscribed === null}
            onCheckedChange={(next) => void toggle(next)}
          />
        }
      />
      <Row title="Play a sound" trailing={<Switch label="Play a sound for approvals" checked={prefs.approval_sound} onCheckedChange={(approval_sound) => void setPreferences({ approval_sound })} />} />
    </Section>
  );
}

function ShortcutsPane() {
  const mod = isMacKeyboard ? '⌘' : 'Ctrl+';
  const groups: { title: string; items: [string, string][] }[] = [
    {
      title: 'Everywhere',
      items: [
        ['Search Macs and pages', `${mod}K`],
        ['Settings', `${mod},`],
      ],
    },
    {
      title: 'On a Mac’s Screen',
      items: [
        ['Paste into the Mac', `${mod}V`],
        ['Copy from the Mac', `${mod}C`],
      ],
    },
  ];
  return (
    <>
      {groups.map((group) => (
        <Section key={group.title} title={group.title}>
          {group.items.map(([label, keys]) => (
            <Row key={label} title={label} trailing={<kbd className={styles.kbd}>{keys}</kbd>} />
          ))}
        </Section>
      ))}
    </>
  );
}
