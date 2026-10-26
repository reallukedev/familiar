// Mouse and keyboard input as CoreGraphics events. Input (argv[0], JSON):
//   { steps: [ step, ... ] } with each step one of
//   { type: "wait_front", app }                 wait until app (name or bundle id) is in front
//   { type: "move", x, y }
//   { type: "click", x, y, button: "left"|"right"|"middle", count, flags }
//   { type: "drag", from_x, from_y, to_x, to_y }
//   { type: "scroll", x, y, dx, dy }              lines; positive dy scrolls content down
//   { type: "text", text }                        any Unicode
//   { type: "key", code, flags }                  virtual key code + modifier flags
// Output: { ok: true }
ObjC.import('AppKit');
ObjC.import('ApplicationServices');
ObjC.import('CoreGraphics');
ObjC.import('Foundation');

// The bridged signature takes UniChar* as a JS string and only passes the first
// character; binding it with raw pointers lets us hand over UTF-16 bytes from NSData.
ObjC.bindFunction('CGEventKeyboardSetUnicodeString', ['void', ['void *', 'unsigned long', 'void *']]);

const HID_TAP = 0;
const CLICK_STATE = 1;
const SCROLL_UNIT_LINE = 1;
const BUTTONS = {
  left: { down: 1, up: 2, drag: 6, button: 0 },
  right: { down: 3, up: 4, drag: 7, button: 1 },
  middle: { down: 25, up: 26, drag: 27, button: 2 },
};
const MOUSE_MOVED = 5;
const KEY_RETURN = 36;
const KEY_TAB = 48;
// Most apps drop events posted faster than they can process them.
const EVENT_GAP = 0.008;

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

function requireTrust() {
  if (!$.AXIsProcessTrusted()) {
    fail('PERMISSION_DENIED', 'macOS blocks synthetic clicks and typing until the agent has Accessibility.');
  }
}

function post(e) {
  $.CGEventPost(HID_TAP, e);
}

function mouse(type, x, y, button, clicks, flags) {
  const e = $.CGEventCreateMouseEvent(null, type, $.CGPointMake(x, y), button);
  if (clicks) $.CGEventSetIntegerValueField(e, CLICK_STATE, clicks);
  $.CGEventSetFlags(e, flags || 0);
  post(e);
}

function move(x, y) {
  mouse(MOUSE_MOVED, x, y, 0, 0, 0);
  delay(0.02);
}

function click(s) {
  const b = BUTTONS[s.button] || BUTTONS.left;
  move(s.x, s.y);
  for (let i = 1; i <= s.count; i++) {
    mouse(b.down, s.x, s.y, b.button, i, s.flags);
    delay(EVENT_GAP);
    mouse(b.up, s.x, s.y, b.button, i, s.flags);
    delay(EVENT_GAP);
  }
}

function drag(s) {
  const b = BUTTONS.left;
  move(s.from_x, s.from_y);
  mouse(b.down, s.from_x, s.from_y, b.button, 1, 0);
  delay(0.05);
  const steps = 12;
  for (let i = 1; i <= steps; i++) {
    const x = s.from_x + ((s.to_x - s.from_x) * i) / steps;
    const y = s.from_y + ((s.to_y - s.from_y) * i) / steps;
    mouse(b.drag, x, y, b.button, 1, 0);
    delay(0.016);
  }
  delay(0.05);
  mouse(b.up, s.to_x, s.to_y, b.button, 1, 0);
}

function scroll(s) {
  move(s.x, s.y);
  // CoreGraphics counts positive wheel values as scrolling up.
  const e = $.CGEventCreateScrollWheelEvent2(null, SCROLL_UNIT_LINE, 2, -s.dy, -s.dx, 0);
  post(e);
}

function key(code, flags) {
  const down = $.CGEventCreateKeyboardEvent(null, code, true);
  $.CGEventSetFlags(down, flags || 0);
  post(down);
  delay(EVENT_GAP);
  const up = $.CGEventCreateKeyboardEvent(null, code, false);
  $.CGEventSetFlags(up, flags || 0);
  post(up);
  delay(EVENT_GAP);
}

function unicode(chunk) {
  const data = $.NSString.alloc.initWithString(chunk).dataUsingEncoding($.NSUTF16LittleEndianStringEncoding);
  const units = Number(data.length) / 2;
  for (const isDown of [true, false]) {
    const e = $.CGEventCreateKeyboardEvent(null, 0, isDown);
    $.CGEventSetFlags(e, 0);
    $.CGEventKeyboardSetUnicodeString(e, units, data.bytes);
    post(e);
    delay(EVENT_GAP);
  }
}

// One event carries at most 20 UTF-16 units; never split a surrogate pair. Newlines and
// tabs are sent as real keys so text fields and editors treat them normally.
function text(s) {
  let chunk = '';
  const flush = () => {
    if (chunk) unicode(chunk);
    chunk = '';
  };
  for (const ch of s) {
    if (ch === '\n' || ch === '\r') { flush(); key(KEY_RETURN, 0); continue; }
    if (ch === '\t') { flush(); key(KEY_TAB, 0); continue; }
    if (chunk.length + ch.length > 20) flush();
    chunk += ch;
  }
  flush();
}

function frontMatches(app) {
  const a = $.NSWorkspace.sharedWorkspace.frontmostApplication;
  if (!a || a.isNil()) return false;
  const wanted = app.toLowerCase();
  const name = a.localizedName.isNil() ? '' : a.localizedName.js.toLowerCase();
  const id = a.bundleIdentifier.isNil() ? '' : a.bundleIdentifier.js.toLowerCase();
  return name === wanted || id === wanted;
}

function waitFront(app) {
  for (let i = 0; i < 40; i++) {
    if (frontMatches(app)) return;
    delay(0.05);
  }
  fail('FAILED', '"' + app + '" did not come to the front. It may be showing a dialog in another Space.');
}

function run(argv) {
  const input = JSON.parse(argv[0]);
  requireTrust();
  for (const s of input.steps) {
    switch (s.type) {
      case 'wait_front': waitFront(s.app); break;
      case 'move': move(s.x, s.y); break;
      case 'click': click(s); break;
      case 'drag': drag(s); break;
      case 'scroll': scroll(s); break;
      case 'text': text(s.text); break;
      case 'key': key(s.code, s.flags); break;
      default: fail('INVALID_ARGS', 'Unknown input step ' + s.type + '.');
    }
  }
  return JSON.stringify({ ok: true });
}
