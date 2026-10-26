import RFB from '@novnc/novnc';
import { useEffect, useImperativeHandle, useRef, type Ref } from 'react';
import { api } from '../lib/api';
import { backoff } from '../lib/backoff';
import { clipboardIntent, typeText } from '../lib/keymap';
import { isMacKeyboard } from '../lib/platform';
import { notify } from '../lib/toast';
import { transport } from '../lib/transport';
import { KEY, sendCombo, type KeyName } from './keys';
import styles from './LiveScreen.module.css';

export type LiveState = { phase: 'connecting' | 'connected' | 'reconnecting' | 'failed'; detail?: string; attempt?: number };
export type LiveScreenHandle = { sendKeys: (keys: KeyName[]) => void; paste: (text: string) => void; focus: () => void; reconnect: () => void };

type Props = {
  instanceId: string;
  viewOnly: boolean;
  onState: (state: LiveState) => void;
  ref?: Ref<LiveScreenHandle>;
  label: string;
  /** Share the clipboard with the Mac while in control (⌘C/⌘V, or Ctrl when it stands in for ⌘). */
  clipboard?: { enabled: boolean; mapCtrl: boolean };
};

const MAX_ATTEMPTS = 8;

/**
 * The Mac's live screen over VNC (noVNC), scaled to fit. View only unless the
 * owner takes control. Reconnects with backoff when the connection drops.
 */
export default function LiveScreen({ instanceId, viewOnly, onState, ref, label, clipboard }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const rfb = useRef<RFB | null>(null);
  const viewOnlyRef = useRef(viewOnly);
  const onStateRef = useRef(onState);
  const restart = useRef<() => void>(() => {});
  useEffect(() => {
    onStateRef.current = onState;
  });

  useEffect(() => {
    const target = host.current;
    if (!target) return;
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let stopped = false;

    const connect = () => {
      if (stopped) return;
      onStateRef.current({ phase: attempt ? 'reconnecting' : 'connecting', attempt });
      const client = new RFB(target, transport.screen(instanceId), { shared: true, wsProtocols: ['binary'] });
      client.scaleViewport = true;
      client.resizeSession = false;
      client.viewOnly = viewOnlyRef.current;
      client.focusOnClick = true;
      client.background = '#0b0b0d';
      client.qualityLevel = 7;
      client.addEventListener('connect', () => {
        attempt = 0;
        onStateRef.current({ phase: 'connected' });
      });
      client.addEventListener('disconnect', (event) => {
        // An older connection closing late mustn't drop the one that replaced it.
        if (rfb.current === client) rfb.current = null;
        if (stopped) return;
        const clean = (event as CustomEvent<{ clean: boolean }>).detail?.clean;
        if (attempt >= MAX_ATTEMPTS) {
          onStateRef.current({ phase: 'failed', detail: 'The screen connection keeps dropping. Check that the Mac is running, then try again.' });
          return;
        }
        const delay = backoff(attempt, Math.random, clean ? 1500 : 1000, 20_000);
        attempt += 1;
        onStateRef.current({ phase: 'reconnecting', attempt });
        timer = setTimeout(connect, delay);
      });
      client.addEventListener('securityfailure', () => {
        stopped = true;
        onStateRef.current({ phase: 'failed', detail: 'Familiar refused the screen connection. Sign in again and reopen the screen.' });
      });
      rfb.current = client;
    };

    restart.current = () => {
      if (timer) clearTimeout(timer);
      stopped = false;
      attempt = 0;
      rfb.current?.disconnect();
      if (!rfb.current) connect();
    };

    connect();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
      rfb.current?.disconnect();
      rfb.current = null;
    };
  }, [instanceId]);

  useEffect(() => {
    viewOnlyRef.current = viewOnly;
    if (rfb.current) rfb.current.viewOnly = viewOnly;
    if (!viewOnly) rfb.current?.focus({ preventScroll: true });
  }, [viewOnly]);

  // QEMU's VNC can't reach the macOS pasteboard, so the clipboard goes through the agent:
  // pasting copies this browser's clipboard to the Mac first, and copying on the Mac
  // brings its clipboard back here.
  const clipboardRef = useRef(clipboard);
  useEffect(() => {
    clipboardRef.current = clipboard;
  });
  useEffect(() => {
    const target = host.current;
    if (!target) return;
    const swallowed = new Set<string>();
    const onKeyDown = (event: KeyboardEvent) => {
      const options = clipboardRef.current;
      const client = rfb.current;
      if (!options?.enabled || viewOnlyRef.current || !client) return;
      const intent = clipboardIntent(event, { macKeyboard: isMacKeyboard, mapCtrl: options.mapCtrl });
      if (intent === 'paste') {
        event.preventDefault();
        event.stopPropagation();
        swallowed.add(event.code);
        void pasteIntoMac(instanceId, client, isMacKeyboard);
      } else if (intent === 'copy') {
        // Let the shortcut reach the Mac, then read what it copied. The clipboard write
        // starts now, inside the key press, so Safari allows it.
        copyFromMac(instanceId);
      }
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (!swallowed.delete(event.code)) return;
      event.preventDefault();
      event.stopPropagation();
    };
    target.addEventListener('keydown', onKeyDown, true);
    target.addEventListener('keyup', onKeyUp, true);
    return () => {
      target.removeEventListener('keydown', onKeyDown, true);
      target.removeEventListener('keyup', onKeyUp, true);
    };
  }, [instanceId]);

  useImperativeHandle(
    ref,
    () => ({
      sendKeys: (keys) => {
        const client = rfb.current;
        if (client) sendCombo((keysym, code, down) => client.sendKey(keysym, code, down), keys);
      },
      paste: (text) => rfb.current?.clipboardPasteFrom(text),
      focus: () => rfb.current?.focus({ preventScroll: true }),
      reconnect: () => restart.current(),
    }),
    [],
  );

  return <div ref={host} className={`${styles.host} ${viewOnly ? styles.viewOnly : ''}`} role="application" aria-label={label} aria-roledescription="remote screen" />;
}

/** Longest text typed out key by key when the Mac's agent can't take the clipboard. */
const TYPE_LIMIT = 2000;

async function pasteIntoMac(instanceId: string, client: RFB, macKeyboard: boolean) {
  let text: string;
  try {
    text = await navigator.clipboard.readText();
  } catch {
    notify.error('Allow clipboard access in your browser to paste into the Mac.');
    return;
  }
  if (!text) return;
  const send = (keysym: number, code: string | null, down: boolean) => client.sendKey(keysym, code, down);
  try {
    await api.setClipboard(instanceId, text);
  } catch {
    // No agent yet (the login window, say): type it instead.
    if (text.length > TYPE_LIMIT) {
      notify.error('The Mac can’t take a paste that long until its agent is running.');
      return;
    }
    if (!macKeyboard) send(KEY.control[0], KEY.control[1], false);
    typeText(send, text);
    return;
  }
  // ⌘ is already held on a Mac keyboard; elsewhere Ctrl is, so swap it for ⌘.
  if (macKeyboard) {
    send(KEY.v[0], KEY.v[1], true);
    send(KEY.v[0], KEY.v[1], false);
  } else {
    send(KEY.control[0], KEY.control[1], false);
    sendCombo(send, ['command', 'v']);
  }
}

function copyFromMac(instanceId: string) {
  const text = new Promise<string>((resolve, reject) => {
    // Give the Mac a moment to finish copying.
    setTimeout(() => {
      api.clipboard(instanceId).then(({ text }) => (text ? resolve(text) : reject(new Error('empty'))), reject);
    }, 350);
  });
  try {
    if (typeof ClipboardItem !== 'undefined' && navigator.clipboard?.write) {
      void navigator.clipboard.write([new ClipboardItem({ 'text/plain': text.then((t) => new Blob([t], { type: 'text/plain' })) })]).catch(() => {});
    } else {
      void text.then((t) => navigator.clipboard.writeText(t)).catch(() => {});
    }
  } catch {
    // Clipboard writes aren't available here; copying stays on the Mac.
  }
}
