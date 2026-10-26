// X11 keysyms for the keys the screen sends on its own. Command is the
// "Super" (left GUI) key on the USB keyboard QEMU gives the Mac; Option is Alt.
export const KEY = {
  command: [0xffeb, 'MetaLeft'],
  option: [0xffe9, 'AltLeft'],
  control: [0xffe3, 'ControlLeft'],
  shift: [0xffe1, 'ShiftLeft'],
  escape: [0xff1b, 'Escape'],
  tab: [0xff09, 'Tab'],
  space: [0x0020, 'Space'],
  return: [0xff0d, 'Enter'],
  backspace: [0xff08, 'Backspace'],
  delete: [0xffff, 'Delete'],
  left: [0xff51, 'ArrowLeft'],
  up: [0xff52, 'ArrowUp'],
  right: [0xff53, 'ArrowRight'],
  down: [0xff54, 'ArrowDown'],
  c: [0x0063, 'KeyC'],
  h: [0x0068, 'KeyH'],
  q: [0x0071, 'KeyQ'],
  v: [0x0076, 'KeyV'],
  w: [0x0077, 'KeyW'],
  three: [0x0033, 'Digit3'],
  five: [0x0035, 'Digit5'],
} as const satisfies Record<string, readonly [number, string]>;

export type KeyName = keyof typeof KEY;

export type Shortcut = { id: string; label: string; hint: string; keys: KeyName[]; separated?: boolean };

export const SHORTCUTS: Shortcut[] = [
  { id: 'switch', label: 'Switch Apps', hint: '⌘⇥', keys: ['command', 'tab'] },
  { id: 'spotlight', label: 'Spotlight', hint: '⌘Space', keys: ['command', 'space'] },
  { id: 'mission', label: 'Mission Control', hint: '⌃↑', keys: ['control', 'up'] },
  { id: 'close', label: 'Close Window', hint: '⌘W', keys: ['command', 'w'], separated: true },
  { id: 'hide', label: 'Hide App', hint: '⌘H', keys: ['command', 'h'] },
  { id: 'quit', label: 'Quit App', hint: '⌘Q', keys: ['command', 'q'] },
  { id: 'force-quit', label: 'Force Quit…', hint: '⌥⌘⎋', keys: ['option', 'command', 'escape'] },
  { id: 'screenshot', label: 'Screenshot', hint: '⇧⌘3', keys: ['shift', 'command', 'three'], separated: true },
  { id: 'screenshot-tools', label: 'Screenshot Tools', hint: '⇧⌘5', keys: ['shift', 'command', 'five'] },
  { id: 'lock', label: 'Lock Screen', hint: '⌃⌘Q', keys: ['control', 'command', 'q'], separated: true },
];

/** Press keys in order and release them in reverse, as a person would. */
export function sendCombo(send: (keysym: number, code: string, down: boolean) => void, keys: KeyName[]) {
  for (const name of keys) send(KEY[name][0], KEY[name][1], true);
  for (const name of keys.toReversed()) send(KEY[name][0], KEY[name][1], false);
}
