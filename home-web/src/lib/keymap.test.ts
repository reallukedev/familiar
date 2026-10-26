import { describe, expect, it } from 'vitest';
import { clipboardIntent, releaseChord, remapsToCommand, strokeFor, typeText, type KeyLike } from './keymap';

const key = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({ code, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
const pc = { macKeyboard: false, mapCtrl: true };
const mac = { macKeyboard: true, mapCtrl: true };

describe('Ctrl to ⌘ on Windows and Linux keyboards', () => {
  it('sends the everyday shortcuts as ⌘, Shift included', () => {
    for (const code of ['KeyC', 'KeyV', 'KeyX', 'KeyZ', 'KeyA', 'KeyS', 'KeyF', 'KeyT', 'KeyW', 'KeyQ', 'KeyN', 'KeyL', 'KeyR']) {
      expect(remapsToCommand(key(code, { ctrlKey: true }), pc), code).toBe(true);
    }
    expect(remapsToCommand(key('KeyZ', { ctrlKey: true, shiftKey: true }), pc)).toBe(true);
  });

  it('leaves other Ctrl keys alone, so Ctrl+D still reaches a terminal', () => {
    expect(remapsToCommand(key('KeyD', { ctrlKey: true }), pc)).toBe(false);
    expect(remapsToCommand(key('KeyC'), pc)).toBe(false);
  });

  it('never touches AltGr (Ctrl+Alt) or a Mac keyboard, and stops when turned off', () => {
    expect(remapsToCommand(key('KeyQ', { ctrlKey: true, altKey: true }), pc)).toBe(false);
    expect(remapsToCommand(key('KeyC', { ctrlKey: true }), mac)).toBe(false);
    expect(remapsToCommand(key('KeyC', { ctrlKey: true }), { macKeyboard: false, mapCtrl: false })).toBe(false);
  });
});

describe('the person’s own copy and paste shortcuts', () => {
  it('reads ⌘V and ⌘C on a Mac keyboard, not Ctrl', () => {
    expect(clipboardIntent(key('KeyV', { metaKey: true }), mac)).toBe('paste');
    expect(clipboardIntent(key('KeyC', { metaKey: true }), mac)).toBe('copy');
    expect(clipboardIntent(key('KeyX', { metaKey: true }), mac)).toBe('copy');
    expect(clipboardIntent(key('KeyV', { ctrlKey: true }), mac)).toBeNull();
  });

  it('reads Ctrl+V and Ctrl+C elsewhere, only while Ctrl stands in for ⌘', () => {
    expect(clipboardIntent(key('KeyV', { ctrlKey: true }), pc)).toBe('paste');
    expect(clipboardIntent(key('KeyC', { ctrlKey: true }), pc)).toBe('copy');
    expect(clipboardIntent(key('KeyV', { ctrlKey: true }), { macKeyboard: false, mapCtrl: false })).toBeNull();
  });

  it('ignores Option combinations such as Paste and Match Style', () => {
    expect(clipboardIntent(key('KeyV', { metaKey: true, altKey: true, shiftKey: true }), mac)).toBeNull();
  });
});

describe('the release chord', () => {
  const press = (tracker: ReturnType<typeof releaseChord>, downs: string[], ups: string[]) => {
    downs.forEach((code) => tracker.down(key(code)));
    return ups.map((code) => tracker.up(key(code)));
  };

  it('fires once when ⌃⌘ is pressed and let go on a Mac keyboard', () => {
    const tracker = releaseChord(true);
    expect(press(tracker, ['ControlLeft', 'MetaLeft'], ['MetaLeft', 'ControlLeft'])).toEqual([true, false]);
  });

  it('uses Ctrl+Alt on other keyboards', () => {
    const tracker = releaseChord(false);
    expect(press(tracker, ['ControlLeft', 'AltRight'], ['ControlLeft', 'AltRight'])).toEqual([true, false]);
    expect(press(releaseChord(false), ['ControlLeft', 'MetaLeft'], ['MetaLeft', 'ControlLeft'])).toEqual([false, false]);
  });

  it('stays quiet when the chord was part of a shortcut, then works again next time', () => {
    const tracker = releaseChord(true);
    expect(press(tracker, ['ControlLeft', 'MetaLeft', 'KeyQ'], ['KeyQ', 'MetaLeft', 'ControlLeft'])).toEqual([false, false, false]);
    expect(press(tracker, ['ShiftLeft', 'ControlLeft', 'MetaLeft'], ['MetaLeft', 'ControlLeft', 'ShiftLeft'])).toEqual([false, false, false]);
    expect(press(tracker, ['MetaLeft', 'ControlLeft'], ['ControlLeft', 'MetaLeft'])).toEqual([true, false]);
  });
});

describe('typing text on the Mac', () => {
  it('uses the physical US key and adds Shift where needed', () => {
    expect(strokeFor('a')).toEqual({ keysym: 0x61, code: 'KeyA', shift: false });
    expect(strokeFor('A')).toEqual({ keysym: 0x41, code: 'KeyA', shift: true });
    expect(strokeFor('?')).toEqual({ keysym: 0x3f, code: 'Slash', shift: true });
    expect(strokeFor('7')).toEqual({ keysym: 0x37, code: 'Digit7', shift: false });
    expect(strokeFor('\n')).toMatchObject({ keysym: 0xff0d, code: 'Enter' });
  });

  it('sends characters beyond the layout as keysyms without a key', () => {
    expect(strokeFor('é')).toEqual({ keysym: 0xe9, code: null, shift: false });
    expect(strokeFor('€')).toEqual({ keysym: 0x010020ac, code: null, shift: false });
  });

  it('presses and releases every key in order, wrapping shifted ones in Shift', () => {
    const log: string[] = [];
    typeText((keysym, code, down) => log.push(`${down ? '+' : '-'}${code ?? keysym.toString(16)}`), 'Hi!');
    expect(log).toEqual(['+ShiftLeft', '+KeyH', '-KeyH', '-ShiftLeft', '+KeyI', '-KeyI', '+ShiftLeft', '+Digit1', '-Digit1', '-ShiftLeft']);
  });
});
