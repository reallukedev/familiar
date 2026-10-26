import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { Button, IconButton } from '../../components/Button';
import { Icon } from '../../components/Icon';
import type { LiveScreenHandle, LiveState } from '../../components/LiveScreen';
import { MacScreen } from '../../components/MacScreen';
import { ActionMenu } from '../../components/Menu';
import { Segmented } from '../../components/Segmented';
import { Spinner } from '../../components/Spinner';
import { Tip } from '../../components/Tip';
import { SHORTCUTS } from '../../components/keys';
import { usePlatform } from '../../lib/platform';
import { usePreferences } from '../../lib/preferences';
import { macPath } from '../../lib/routes';
import { hasScreen } from '../../lib/status';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import { usePower } from './usePower';
import styles from './Screen.module.css';
import { PowerConfirm } from './PowerMenu';

const LiveScreen = lazy(() => import('../../components/LiveScreen'));

type Mode = 'view' | 'control';

/** The Mac's screen, live. View only until the owner takes control. */
export default function ScreenPage({ instance, back }: MacPageProps & { back?: { href: string; label: string } }) {
  const phone = usePlatform() === 'phone';
  const prefs = usePreferences();
  const [mode, setMode] = useState<Mode>('view');
  const [state, setState] = useState<LiveState>({ phase: 'connecting' });
  const [fullscreen, setFullscreen] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const screen = useRef<LiveScreenHandle>(null);
  const power = usePower(instance);
  const running = hasScreen(instance.state) && instance.state !== 'booting';

  useEffect(() => {
    const onChange = () => setFullscreen(document.fullscreenElement === stage.current);
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  const toggleFullscreen = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void stage.current?.requestFullscreen?.().catch(() => {});
  };

  const controls = running && instance.driver === 'qemu' ? (
    <>
      <Segmented
        label="Screen mode"
        size="compact"
        value={mode}
        onChange={setMode}
        options={[
          { value: 'view', label: 'View Only' },
          { value: 'control', label: phone ? 'Control' : 'Take Control' },
        ]}
      />
      <ActionMenu
        label="Send keys to the Mac"
        trigger={
          <IconButton label="Send keys to the Mac" disabled={state.phase !== 'connected' || mode !== 'control'}>
            <Icon name="keyboard" size={19} weight={1.8} />
          </IconButton>
        }
        items={SHORTCUTS.map((shortcut) => ({ label: shortcut.label, hint: shortcut.hint, onSelect: () => screen.current?.sendKeys(shortcut.keys) }))}
      />
      {document.fullscreenEnabled ? (
        <Tip label={fullscreen ? 'Exit Full Screen' : 'Enter Full Screen'}>
          <IconButton label={fullscreen ? 'Exit full screen' : 'Enter full screen'} onClick={toggleFullscreen}>
            <Icon name={fullscreen ? 'collapse' : 'expand'} size={18} weight={1.9} />
          </IconButton>
        </Tip>
      ) : null}
    </>
  ) : undefined;

  return (
    <Page title={phone ? "Screen" : instance.name} back={back ?? { href: macPath(instance.id), label: instance.name }} width="full" actions={phone ? undefined : controls}>
      <div className={styles.wrap}>
        {instance.driver === 'remote' ? (
          <div className={styles.notice}>
            <MacScreen instance={instance} size="hero" />
            <p className="t-subhead secondary pretty">The live screen is for Macs on this server. For a Mac on your network, use Screen Sharing from another Mac.</p>
          </div>
        ) : !running ? (
          <div className={styles.notice}>
            <MacScreen instance={instance} size="hero" onPower={power.run} powerBusy={!!power.busy} />
            <PowerConfirm instance={instance} power={power} />
          </div>
        ) : (
          <div ref={stage} className={`${styles.stage} ${mode === 'control' ? styles.control : ''}`}>
            <div className={styles.area}>
            <div className={styles.bezel}>
              <Suspense fallback={<div className={styles.connecting}><Spinner size={22} /></div>}>
                <LiveScreen ref={screen} instanceId={instance.id} viewOnly={mode === 'view'} onState={setState} clipboard={{ enabled: prefs.clipboard_sync, mapCtrl: prefs.map_ctrl_to_cmd }} label={`${instance.name} screen, ${mode === 'view' ? 'view only' : 'you are in control'}`} />
              </Suspense>
              {state.phase !== 'connected' ? (
                <div className={styles.overlay} role="status">
                  {state.phase === 'failed' ? (
                    <>
                      <Icon name="wifi" size={26} weight={1.8} />
                      <p className={styles.overlayText}>{state.detail}</p>
                      <Button variant="on-dark" onClick={() => screen.current?.reconnect()}>
                        Try Again
                      </Button>
                    </>
                  ) : (
                    <>
                      <Spinner size={22} />
                      <p className={styles.overlayText}>{state.phase === 'reconnecting' ? 'Reconnecting to the screen…' : 'Connecting to the screen…'}</p>
                    </>
                  )}
                </div>
              ) : null}
            </div>
            </div>
            {phone ? <div className={styles.phoneBar}>{controls}</div> : null}
            <p className={`t-footnote ${styles.hint}`} aria-live="polite">
              {mode === 'control' ? 'You’re in control.' : 'View only'}
            </p>
          </div>
        )}
      </div>
    </Page>
  );
}
