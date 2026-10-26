// Translating the person's keyboard into the Mac's. Pure, so it is unit tested;
// the screen calls these before noVNC sees a key.

export type KeyLike = { code: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean };
/** One key press for the Mac: an X11 keysym, the physical key when known, and whether Shift goes with it. */
export type Stroke = { keysym: number; code: string | null; shift: boolean };
export type Send = (keysym: number, code: string | null, down: boolean) => void;

// The everyday shortcuts people type with Ctrl on Windows and Linux and with ⌘ on a Mac.
const REMAPPED = new Set(['KeyA', 'KeyC', 'KeyF', 'KeyL', 'KeyN', 'KeyQ', 'KeyR', 'KeyS', 'KeyT', 'KeyV', 'KeyW', 'KeyX', 'KeyZ']);

type Options = { macKeyboard: boolean; mapCtrl: boolean };

/** Ctrl+C typed on a Windows or Linux keyboard should reach the Mac as ⌘C. AltGr (Ctrl+Alt) is left alone. */
export function remapsToCommand(event: KeyLike, { macKeyboard, mapCtrl }: Options): boolean {
  return mapCtrl && !macKeyboard && event.ctrlKey && !event.metaKey && !event.altKey && REMAPPED.has(event.code);
}

/**
 * Whether the person just pressed their own paste or copy shortcut: ⌘V and ⌘C on
 * a Mac keyboard, Ctrl+V and Ctrl+C elsewhere (only while Ctrl stands in for ⌘).
 */
export function clipboardIntent(event: KeyLike, { macKeyboard, mapCtrl }: Options): 'paste' | 'copy' | null {
  if (!macKeyboard && !mapCtrl) return null;
  const primary = macKeyboard ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!primary || event.altKey) return null;
  if (event.code === 'KeyV') return 'paste';
  if (event.code === 'KeyC' || event.code === 'KeyX') return 'copy';
  return null;
}

/** Shown in tooltips and hints: the chord that hands the keyboard back to the page. */
export function releaseChordLabel(macKeyboard: boolean): string {
  return macKeyboard ? '⌃⌘' : 'Ctrl+Alt';
}

function modifierOf(code: string): string | null {
  const match = /^(Control|Meta|Alt|Shift|OS)(Left|Right)?$/.exec(code);
  if (!match) return null;
  return match[1] === 'OS' ? 'Meta' : match[1]!;
}

/**
 * Notices the release chord: Control and ⌘ (Control and Alt on other keyboards)
 * pressed together and let go with nothing else typed in between, like VMware.
 */
export function releaseChord(macKeyboard: boolean) {
  const chord = macKeyboard ? ['Control', 'Meta'] : ['Control', 'Alt'];
  const held = new Set<string>();
  let armed = false;
  let spoiled = false;
  return {
    down(event: KeyLike) {
      const modifier = modifierOf(event.code);
      if (!modifier) {
        spoiled = true;
        return;
      }
      held.add(modifier);
      if (held.size === chord.length && chord.every((key) => held.has(key))) armed = true;
      else if (held.size > chord.length || !chord.includes(modifier)) spoiled = true;
    },
    /** True when this release completes the chord. */
    up(event: KeyLike): boolean {
      const modifier = modifierOf(event.code);
      const fired = armed && !spoiled && !!modifier && chord.includes(modifier);
      if (modifier) held.delete(modifier);
      if (fired || held.size === 0) {
        armed = false;
        spoiled = held.size > 0;
      }
      return fired;
    },
  };
}

// US layout: the unshifted key behind each printable character.
const KEY_CODES: Record<string, string> = {
  '1': 'Digit1', '2': 'Digit2', '3': 'Digit3', '4': 'Digit4', '5': 'Digit5',
  '6': 'Digit6', '7': 'Digit7', '8': 'Digit8', '9': 'Digit9', '0': 'Digit0',
  '-': 'Minus', '=': 'Equal', '[': 'BracketLeft', ']': 'BracketRight', '\\': 'Backslash',
  ';': 'Semicolon', "'": 'Quote', ',': 'Comma', '.': 'Period', '/': 'Slash', '`': 'Backquote', ' ': 'Space',
};
const SHIFTED: Record<string, string> = {
  '!': '1', '@': '2', '#': '3', $: '4', '%': '5', '^': '6', '&': '7', '*': '8', '(': '9', ')': '0',
  _: '-', '+': '=', '{': '[', '}': ']', '|': '\\', ':': ';', '"': "'", '<': ',', '>': '.', '?': '/', '~': '`',
};

/** How to type one character on the Mac. Characters beyond the US layout go as bare keysyms. */
export function strokeFor(char: string): Stroke {
  if (char === '\n' || char === '\r') return { keysym: 0xff0d, code: 'Enter', shift: false };
  if (char === '\t') return { keysym: 0xff09, code: 'Tab', shift: false };
  if (/^[a-z]$/.test(char)) return { keysym: char.charCodeAt(0), code: `Key${char.toUpperCase()}`, shift: false };
  if (/^[A-Z]$/.test(char)) return { keysym: char.charCodeAt(0), code: `Key${char}`, shift: true };
  const plain = KEY_CODES[char];
  if (plain) return { keysym: char.charCodeAt(0), code: plain, shift: false };
  const base = SHIFTED[char];
  if (base) return { keysym: char.charCodeAt(0), code: KEY_CODES[base]!, shift: true };
  const point = char.codePointAt(0) ?? 0x3f;
  // Latin-1 keysyms equal their code points; the rest use the Unicode keysym range.
  return { keysym: point <= 0xff ? point : 0x01000000 + point, code: null, shift: false };
}

const SHIFT = 0xffe1;

/** Type text on the Mac one key at a time, pressing Shift where the US layout needs it. */
export function typeText(send: Send, text: string) {
  for (const char of text.replace(/\r\n/g, '\n')) {
    const stroke = strokeFor(char);
    if (stroke.shift) send(SHIFT, 'ShiftLeft', true);
    send(stroke.keysym, stroke.code, true);
    send(stroke.keysym, stroke.code, false);
    if (stroke.shift) send(SHIFT, 'ShiftLeft', false);
  }
}
