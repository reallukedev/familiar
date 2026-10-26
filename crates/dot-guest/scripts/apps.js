// App information and control through NSWorkspace, NSRunningApplication and the AX API.
// No Apple events, so no Automation prompts.
// Input (argv[0], JSON):
//   { mode: "list", paths: [String], running_only }  -> [AppInfo]
//   { mode: "resolve", app }                          -> AppInfo  (NOT_FOUND if unknown)
//   { mode: "quit", app, force, wait_ms }             -> { quit: true }
//   { mode: "windows", app? }                          -> [WindowInfo]
ObjC.import('AppKit');
ObjC.import('ApplicationServices');

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

function info(path) {
  const bundle = $.NSBundle.bundleWithPath(path);
  if (!bundle || bundle.isNil()) return null;
  const get = (key) => {
    const v = bundle.objectForInfoDictionaryKey(key);
    return !v || v.isNil() || !v.isKindOfClass($.NSString) ? null : v.js;
  };
  const base = path.split('/').pop().replace(/\.app$/, '');
  const id = bundle.bundleIdentifier.isNil() ? null : bundle.bundleIdentifier.js;
  const scriptableFlag = bundle.objectForInfoDictionaryKey('NSAppleScriptEnabled');
  let scriptable = !!get('OSAScriptingDefinition');
  if (!scriptable && scriptableFlag && !scriptableFlag.isNil()) {
    scriptable = scriptableFlag.isKindOfClass($.NSNumber) ? !!scriptableFlag.boolValue
      : String(scriptableFlag.js).toUpperCase() === 'YES';
  }
  return {
    name: get('CFBundleDisplayName') || get('CFBundleName') || base,
    bundle_id: id,
    path: path,
    version: get('CFBundleShortVersionString'),
    running: false,
    scriptable: scriptable,
  };
}

function running() {
  const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
  const out = [];
  for (let i = 0; i < Number(apps.count); i++) out.push(apps.objectAtIndex(i));
  return out;
}

function runningInfo(a) {
  const url = a.bundleURL;
  const path = !url || url.isNil() ? null : url.path.js;
  const base = path ? info(path) : null;
  const name = a.localizedName.isNil() ? null : a.localizedName.js;
  const out = base || {
    name: name || 'Unknown',
    bundle_id: a.bundleIdentifier.isNil() ? null : a.bundleIdentifier.js,
    path: path,
    version: null,
    running: true,
    scriptable: false,
  };
  if (name) out.name = name;
  out.running = true;
  return out;
}

function matches(a, wanted) {
  const name = a.localizedName.isNil() ? '' : a.localizedName.js.toLowerCase();
  const id = a.bundleIdentifier.isNil() ? '' : a.bundleIdentifier.js.toLowerCase();
  return name === wanted || id === wanted;
}

function list(input) {
  const byPath = {};
  const out = [];
  // NSApplicationActivationPolicyRegular: apps with a Dock icon.
  for (const a of running()) {
    if (Number(a.activationPolicy) !== 0) continue;
    const i = runningInfo(a);
    if (i.path) byPath[i.path] = i;
    out.push(i);
  }
  if (!input.running_only) {
    for (const p of input.paths) {
      if (byPath[p]) continue;
      const i = info(p);
      if (i) out.push(i);
    }
  }
  out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

function resolve(app) {
  const wanted = app.toLowerCase();
  for (const a of running()) {
    if (matches(a, wanted)) return runningInfo(a);
  }
  const ws = $.NSWorkspace.sharedWorkspace;
  let path = null;
  if (app.endsWith('.app') && app.startsWith('/')) {
    path = app;
  } else {
    const url = ws.URLForApplicationWithBundleIdentifier(app);
    if (url && !url.isNil()) path = url.path.js;
    if (!path) {
      const full = ws.fullPathForApplication(app);
      if (full && !full.isNil()) path = full.js;
    }
  }
  const i = path ? info(path) : null;
  if (!i) fail('NOT_FOUND', 'No app called "' + app + '" is installed.');
  return i;
}

function quit(input) {
  const wanted = input.app.toLowerCase();
  const targets = running().filter((a) => matches(a, wanted));
  if (targets.length === 0) fail('NOT_FOUND', '"' + input.app + '" is not running.');
  for (const a of targets) {
    if (input.force) a.forceTerminate;
    else a.terminate;
  }
  const deadline = Date.now() + input.wait_ms;
  while (Date.now() < deadline) {
    if (targets.every((a) => a.terminated)) return { quit: true };
    delay(0.1);
  }
  if (input.force) fail('FAILED', '"' + input.app + '" did not stop.');
  fail('FAILED', '"' + input.app + '" did not quit. It may be asking to save changes; take a screenshot, or quit with force to discard them.');
}

function axAttr(el, name) {
  const ref = Ref();
  if (Number($.AXUIElementCopyAttributeValue(el, $(name), ref)) !== 0) return null;
  return ObjC.castRefToObject(ref[0]);
}

function axFrame(o) {
  if (!o || o.isNil()) return null;
  const m = /x:(-?[\d.]+) y:(-?[\d.]+) w:(-?[\d.]+) h:(-?[\d.]+)/.exec(o.description.js);
  return m ? { x: Number(m[1]), y: Number(m[2]), width: Number(m[3]), height: Number(m[4]) } : null;
}

// Windows through the AX API: needs Accessibility only, not Automation.
function windows(input) {
  if (!$.AXIsProcessTrusted()) {
    fail('PERMISSION_DENIED', 'macOS blocked the agent from reading other apps\' windows (Accessibility is off).');
  }
  let apps = running().filter((a) => Number(a.activationPolicy) === 0);
  if (input.app) {
    const wanted = input.app.toLowerCase();
    apps = running().filter((a) => matches(a, wanted));
    if (apps.length === 0) fail('NOT_FOUND', '"' + input.app + '" is not running.');
  }
  const out = [];
  for (const a of apps) {
    const name = a.localizedName.isNil() ? 'Unknown' : a.localizedName.js;
    const el = $.AXUIElementCreateApplication(a.processIdentifier);
    $.AXUIElementSetMessagingTimeout(el, 2.0);
    const wins = axAttr(el, 'AXWindows');
    if (!wins || !wins.isKindOfClass($.NSArray)) continue;
    const focusedWin = a.active ? axAttr(el, 'AXFocusedWindow') : null;
    for (let i = 0; i < Number(wins.count); i++) {
      const w = wins.objectAtIndex(i);
      const role = axAttr(w, 'AXRole');
      if (!role || role.js !== 'AXWindow') continue;
      const title = axAttr(w, 'AXTitle');
      const min = axAttr(w, 'AXMinimized');
      out.push({
        app: name,
        title: title && title.isKindOfClass($.NSString) ? title.js : '',
        frame: axFrame(axAttr(w, 'AXFrame')),
        minimized: !!(min && min.isKindOfClass($.NSNumber) && min.boolValue),
        focused: !!(focusedWin && $.CFEqual(focusedWin, w)),
      });
    }
  }
  return out;
}

function run(argv) {
  const input = JSON.parse(argv[0]);
  switch (input.mode) {
    case 'list': return JSON.stringify(list(input));
    case 'resolve': return JSON.stringify(resolve(input.app));
    case 'quit': return JSON.stringify(quit(input));
    case 'windows': return JSON.stringify(windows(input));
    default: fail('INVALID_ARGS', 'Unknown mode ' + input.mode + '.');
  }
}
