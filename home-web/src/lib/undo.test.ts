import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./preferences', () => ({ currentPreferences: () => ({ undo_seconds: 3 }) }));
vi.mock('./toast', () => {
  let n = 0;
  return { toastManager: { add: () => `t${++n}`, close: () => {} } };
});

// Tests run in Node: a page and its document, just enough for undo's listeners.
const page = new EventTarget();
const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' });
Object.assign(globalThis, {
  window: globalThis,
  document: doc,
  addEventListener: page.addEventListener.bind(page),
  dispatchEvent: page.dispatchEvent.bind(page),
});

const { withUndo } = await import('./undo');

let coarse = false;
function setHidden(hidden: boolean) {
  doc.visibilityState = hidden ? 'hidden' : 'visible';
  doc.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
  vi.useFakeTimers();
  coarse = false;
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' && coarse }));
});
afterEach(() => {
  setHidden(false);
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('withUndo', () => {
  it('runs once the countdown ends', () => {
    const action = vi.fn();
    withUndo('Deleting…', action);
    vi.advanceTimersByTime(2_999);
    expect(action).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(action).toHaveBeenCalledOnce();
  });

  it('runs at once when the page closes', () => {
    const action = vi.fn();
    withUndo('Deleting…', action);
    dispatchEvent(new Event('pagehide'));
    expect(action).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(5_000);
    expect(action).toHaveBeenCalledOnce();
  });

  it('keeps counting when a computer switches tabs', () => {
    const action = vi.fn();
    withUndo('Deleting…', action);
    setHidden(true);
    expect(action).not.toHaveBeenCalled();
    vi.advanceTimersByTime(3_000);
    expect(action).toHaveBeenCalledOnce();
  });

  it('runs at once when a phone leaves the app', () => {
    coarse = true;
    const action = vi.fn();
    withUndo('Deleting…', action);
    setHidden(true);
    expect(action).toHaveBeenCalledOnce();
  });

  it('catches up on return if timers were paused past the deadline', () => {
    const action = vi.fn();
    withUndo('Deleting…', action);
    setHidden(true);
    vi.setSystemTime(Date.now() + 10_000); // the clock moved, but no timer fired
    setHidden(false);
    expect(action).toHaveBeenCalledOnce();
  });
});
