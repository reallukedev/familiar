// Everything a screenshot needs before capturing. Input: { app?: string }.
// Output: { displays: [...], screen_recording: bool|null,
//           window?: { id, owner, x, y, width, height } }
// Throws NOT_FOUND when `app` has no visible window.
ObjC.import('AppKit');
ObjC.import('CoreGraphics');

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

function displays() {
  const screens = $.NSScreen.screens;
  const out = [];
  const count = Number(screens.count);
  if (count === 0) return out;
  const primaryHeight = Number(screens.objectAtIndex(0).frame.size.height);
  for (let i = 0; i < count; i++) {
    const s = screens.objectAtIndex(i);
    const f = s.frame;
    const h = Number(f.size.height);
    out.push({
      index: i + 1,
      x: Number(f.origin.x),
      y: primaryHeight - (Number(f.origin.y) + h),
      width: Number(f.size.width),
      height: h,
      scale: Number(s.backingScaleFactor),
      main: i === 0,
    });
  }
  return out;
}

function screenRecording() {
  try {
    ObjC.bindFunction('CGPreflightScreenCaptureAccess', ['bool', []]);
    return !!$.CGPreflightScreenCaptureAccess();
  } catch (e) {
    return null;
  }
}

// Running apps whose name or bundle id matches, as a set of pids.
function pidsFor(app) {
  const wanted = app.toLowerCase();
  const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
  const pids = {};
  for (let i = 0; i < Number(apps.count); i++) {
    const a = apps.objectAtIndex(i);
    const name = a.localizedName.isNil() ? '' : a.localizedName.js.toLowerCase();
    const id = a.bundleIdentifier.isNil() ? '' : a.bundleIdentifier.js.toLowerCase();
    if (name === wanted || id === wanted) pids[Number(a.processIdentifier)] = true;
  }
  return pids;
}

function frontWindow(app) {
  const pids = pidsFor(app);
  if (Object.keys(pids).length === 0) {
    fail('NOT_FOUND', 'No running app called "' + app + '". Open it first.');
  }
  // kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements; front to back.
  const list = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1 | 16, 0))) || [];
  for (const w of list) {
    if (!pids[w.kCGWindowOwnerPID]) continue;
    if (w.kCGWindowLayer !== 0) continue;
    if (w.kCGWindowAlpha === 0) continue;
    const b = w.kCGWindowBounds || {};
    if (!(b.Width > 1 && b.Height > 1)) continue;
    return { id: w.kCGWindowNumber, owner: w.kCGWindowOwnerName, x: b.X, y: b.Y, width: b.Width, height: b.Height };
  }
  fail('NOT_FOUND', '"' + app + '" has no visible window on screen. It may be minimized or on another Space.');
}

function run(argv) {
  const input = JSON.parse(argv[0] || '{}');
  const out = { displays: displays(), screen_recording: screenRecording() };
  if (input.app) out.window = frontWindow(input.app);
  return JSON.stringify(out);
}
