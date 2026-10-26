// What the agent can see about this Mac. Input: {}. Output:
// { displays: [{index, x, y, width, height, scale, main}], accessibility, screen_recording,
//   frontmost_app, default_browser, uptime_s }
// Uses only the ObjC bridge, so it never sends Apple events or triggers privacy prompts.
ObjC.import('AppKit');
ObjC.import('ApplicationServices');
ObjC.import('CoreGraphics');

function displays() {
  const screens = $.NSScreen.screens;
  const out = [];
  const count = Number(screens.count);
  if (count === 0) return out;
  // Cocoa puts the origin at the bottom-left of the primary screen; convert to top-left.
  const primaryHeight = Number(screens.objectAtIndex(0).frame.size.height);
  for (let i = 0; i < count; i++) {
    const s = screens.objectAtIndex(i);
    const f = s.frame;
    const h = Number(f.size.height);
    out.push({
      index: i + 1,
      x: Number(f.origin.x),
      y: primaryHeight - (Number(f.origin.y) + h),
      width: Math.round(Number(f.size.width)),
      height: Math.round(h),
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

function accessibility() {
  try {
    return !!$.AXIsProcessTrusted();
  } catch (e) {
    return null;
  }
}

function frontmost() {
  const app = $.NSWorkspace.sharedWorkspace.frontmostApplication;
  if (!app || app.isNil()) return null;
  const name = app.localizedName;
  return name.isNil() ? null : name.js;
}

function defaultBrowser() {
  const url = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL(
    $.NSURL.URLWithString('https://example.com')
  );
  if (!url || url.isNil()) return null;
  const bundle = $.NSBundle.bundleWithURL(url);
  if (!bundle || bundle.isNil() || bundle.bundleIdentifier.isNil()) return null;
  return bundle.bundleIdentifier.js;
}

function run(argv) {
  return JSON.stringify({
    displays: displays(),
    accessibility: accessibility(),
    screen_recording: screenRecording(),
    frontmost_app: frontmost(),
    default_browser: defaultBrowser(),
    uptime_s: Math.floor(Number($.NSProcessInfo.processInfo.systemUptime)),
  });
}
