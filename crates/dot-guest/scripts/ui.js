// Accessibility tree, search and element actions through the AX API (ObjC bridge).
// Talking to AX directly is about 30x faster than going through System Events and needs
// no Automation permission, only Accessibility.
//
// Input (argv[0], JSON):
//   { mode: "tree", app?, max_depth, max_nodes, include_menus, budget_ms }
//   { mode: "find", app?, text?, role?, limit, budget_ms }
//   { mode: "action", pid, path, role, action: {type, value?, name?} }
// Output:
//   tree/find: { app, pid, truncated, nodes: [{ path, depth, role, subrole, title, value,
//                description, frame, enabled, focused, actions }] }
//   action:    { ok: true, value?, click?: [x, y] }   (click: AXPress failed, click here)
// A path is ["w" | "m", i0, i1, ...]: window (or menu bar) index, then child indexes.
ObjC.import('AppKit');
ObjC.import('ApplicationServices');

const ATTRS = ['AXRole', 'AXSubrole', 'AXTitle', 'AXValue', 'AXDescription', 'AXFrame',
  'AXEnabled', 'AXFocused', 'AXChildren', 'AXHelp'];
const I = { role: 0, subrole: 1, title: 2, value: 3, description: 4, frame: 5, enabled: 6,
  focused: 7, children: 8, help: 9 };
const MAX_VALUE = 500;

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

function requireTrust() {
  if (!$.AXIsProcessTrusted()) {
    fail('PERMISSION_DENIED', 'macOS blocked the agent from reading other apps (Accessibility is off).');
  }
}

function str(o) {
  if (!o || o.isNil()) return null;
  if (o.isKindOfClass($.NSString)) return o.js;
  return null;
}

function bool(o, fallback) {
  if (!o || o.isNil()) return fallback;
  if (o.isKindOfClass($.NSNumber)) return !!o.boolValue;
  return fallback;
}

// Value as text: strings and numbers only; element references and AX errors are skipped.
function valueText(o) {
  if (!o || o.isNil()) return null;
  let s = null;
  if (o.isKindOfClass($.NSString)) s = o.js;
  else if (o.isKindOfClass($.NSNumber)) s = String(o.js);
  else if (o.isKindOfClass($.NSAttributedString)) s = o.string.js;
  if (s === null) return null;
  return s.length > MAX_VALUE ? s.slice(0, MAX_VALUE) : s;
}

// AXValue structs have no bridge accessor; their description is stable:
// "{value = x:672.000000 y:287.000000 w:920.000000 h:604.000000 type = kAXValueCGRectType}"
function frame(o) {
  if (!o || o.isNil()) return null;
  const d = o.description.js;
  const m = /x:(-?[\d.]+) y:(-?[\d.]+) w:(-?[\d.]+) h:(-?[\d.]+)/.exec(d);
  if (!m) return null;
  return { x: Number(m[1]), y: Number(m[2]), width: Number(m[3]), height: Number(m[4]) };
}

function children(o) {
  if (!o || o.isNil() || !o.isKindOfClass($.NSArray)) return [];
  const out = [];
  for (let i = 0; i < Number(o.count); i++) out.push(o.objectAtIndex(i));
  return out;
}

function copyAttr(el, name) {
  const ref = Ref();
  const err = Number($.AXUIElementCopyAttributeValue(el, $(name), ref));
  if (err !== 0) return { err };
  return { value: ObjC.castRefToObject(ref[0]) };
}

function actionNames(el) {
  const ref = Ref();
  if (Number($.AXUIElementCopyActionNames(el, ref)) !== 0) return [];
  const arr = ObjC.castRefToObject(ref[0]);
  const out = [];
  for (let i = 0; i < Number(arr.count); i++) out.push(arr.objectAtIndex(i).js);
  return out;
}

function readNode(el) {
  const ref = Ref();
  const err = Number($.AXUIElementCopyMultipleAttributeValues(el, $(ATTRS), 0, ref));
  if (err === -25211) requireTrust();
  if (err !== 0) return null;
  const v = ObjC.castRefToObject(ref[0]);
  const at = (i) => v.objectAtIndex(i);
  return {
    role: str(at(I.role)) || 'AXUnknown',
    subrole: str(at(I.subrole)),
    title: str(at(I.title)),
    value: valueText(at(I.value)),
    description: str(at(I.description)),
    help: str(at(I.help)),
    frame: frame(at(I.frame)),
    enabled: bool(at(I.enabled), true),
    focused: bool(at(I.focused), false),
    kids: children(at(I.children)),
  };
}

function runningApps() {
  const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
  const out = [];
  for (let i = 0; i < Number(apps.count); i++) out.push(apps.objectAtIndex(i));
  return out;
}

function appName(a) {
  return a.localizedName.isNil() ? '' : a.localizedName.js;
}

function findApp(input) {
  if (input.pid) {
    const a = $.NSRunningApplication.runningApplicationWithProcessIdentifier(input.pid);
    if (!a || a.isNil() || a.terminated) {
      fail('STALE_REF', 'The app this element belonged to has quit. Read the UI again.');
    }
    return { pid: input.pid, name: appName(a) };
  }
  if (!input.app) {
    const a = $.NSWorkspace.sharedWorkspace.frontmostApplication;
    if (!a || a.isNil()) fail('NOT_FOUND', 'No app is in front.');
    return { pid: Number(a.processIdentifier), name: appName(a) };
  }
  const wanted = input.app.toLowerCase();
  for (const a of runningApps()) {
    const id = a.bundleIdentifier.isNil() ? '' : a.bundleIdentifier.js.toLowerCase();
    if (id === wanted || appName(a).toLowerCase() === wanted) {
      return { pid: Number(a.processIdentifier), name: appName(a) };
    }
  }
  fail('NOT_FOUND', 'No running app called "' + input.app + '". Open it first.');
}

function appElement(pid) {
  const app = $.AXUIElementCreateApplication(pid);
  // Bounded wait for apps that are busy or hung.
  $.AXUIElementSetMessagingTimeout(app, 3.0);
  return app;
}

function roots(app, includeMenus) {
  const out = [];
  const wins = copyAttr(app, 'AXWindows');
  if (wins.err === -25211) requireTrust();
  children(wins.value).forEach((el, i) => out.push({ el, path: ['w', i] }));
  if (includeMenus) {
    const bar = copyAttr(app, 'AXMenuBar');
    if (bar.value && !bar.value.isNil()) out.push({ el: bar.value, path: ['m', 0] });
  }
  return out;
}

// Walk breadth-first so a node limit keeps the top of every window, then return the
// collected nodes in document order.
function walk(app, opts, keep) {
  const deadline = Date.now() + opts.budget_ms;
  const queue = roots(app, opts.include_menus);
  const nodes = [];
  let visited = 0;
  let truncated = false;
  for (let q = 0; q < queue.length; q++) {
    if (Date.now() > deadline || visited >= opts.max_visit) { truncated = true; break; }
    const item = queue[q];
    const node = readNode(item.el);
    visited++;
    if (!node) continue;
    // Finder's desktop and similar pseudo-windows are not windows.
    if (item.path.length === 2 && item.path[0] === 'w' && node.role !== 'AXWindow') continue;
    const depth = item.path.length - 1;
    if (keep(node)) {
      if (nodes.length >= opts.max_nodes) { truncated = true; break; }
      node.path = item.path;
      node.depth = depth;
      node.actions = actionNames(item.el);
      nodes.push(node);
    }
    if (depth < opts.max_depth + 1) {
      node.kids.forEach((el, i) => queue.push({ el, path: item.path.concat([i]) }));
    }
    delete node.kids;
  }
  nodes.sort((a, b) => comparePaths(a.path, b.path));
  for (const n of nodes) delete n.kids;
  return { nodes, truncated };
}

function comparePaths(a, b) {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    if (a[i] === b[i]) continue;
    if (i === 0) return a[i] === 'w' ? -1 : 1;
    return a[i] - b[i];
  }
  return a.length - b.length;
}

function tree(input) {
  const target = findApp(input);
  const app = appElement(target.pid);
  // Electron apps only build their AX tree when asked; other apps ignore this.
  $.AXUIElementSetAttributeValue(app, $('AXManualAccessibility'), $.NSNumber.numberWithBool(true));
  const res = walk(app, {
    budget_ms: input.budget_ms,
    include_menus: !!input.include_menus,
    max_depth: input.max_depth,
    max_nodes: input.max_nodes,
    max_visit: input.max_nodes * 4 + 50,
  }, () => true);
  return { app: target.name, pid: target.pid, truncated: res.truncated, nodes: res.nodes };
}

function find(input) {
  const target = findApp(input);
  const app = appElement(target.pid);
  $.AXUIElementSetAttributeValue(app, $('AXManualAccessibility'), $.NSNumber.numberWithBool(true));
  const text = input.text ? input.text.toLowerCase() : null;
  const role = input.role || null;
  const matches = (n) => {
    if (role && n.role !== role) return false;
    if (!text) return true;
    return [n.title, n.value, n.description, n.help].some((s) => s && s.toLowerCase().indexOf(text) >= 0);
  };
  const res = walk(app, {
    budget_ms: input.budget_ms,
    include_menus: true,
    max_depth: 40,
    max_nodes: input.limit,
    max_visit: 6000,
  }, matches);
  return { app: target.name, pid: target.pid, truncated: res.truncated, nodes: res.nodes };
}

function resolve(app, path) {
  let el;
  if (path[0] === 'm') {
    const bar = copyAttr(app, 'AXMenuBar');
    if (bar.err === -25211) requireTrust();
    el = bar.value;
  } else {
    const wins = copyAttr(app, 'AXWindows');
    if (wins.err === -25211) requireTrust();
    el = children(wins.value)[path[1]];
  }
  for (let i = 2; i < path.length && el; i++) {
    el = children(copyAttr(el, 'AXChildren').value)[path[i]];
  }
  if (!el || el.isNil()) fail('STALE_REF', 'That element is gone. Read the UI again.');
  return el;
}

function checkAx(err, what) {
  err = Number(err);
  if (err === 0) return;
  if (err === -25211) requireTrust();
  if (err === -25202) fail('STALE_REF', 'That element is gone. Read the UI again.');
  if (err === -25206 || err === -25205) fail('INVALID_ARGS', 'This element does not support ' + what + '.');
  if (err === -25204) fail('TIMEOUT', 'The app did not respond to ' + what + ' in time.');
  fail('FAILED', 'The app refused ' + what + ' (AX error ' + err + ').');
}

function activate(pid) {
  const a = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid);
  // NSApplicationActivateIgnoringOtherApps
  if (a && !a.isNil()) a.activateWithOptions(2);
}

function action(input) {
  requireTrust();
  findApp(input);
  const app = appElement(input.pid);
  const el = resolve(app, input.path);
  const node = readNode(el);
  if (!node) fail('STALE_REF', 'That element is gone. Read the UI again.');
  if (node.role !== input.role) {
    fail('STALE_REF', 'The UI changed (expected ' + input.role + ', found ' + node.role + '). Read the UI again.');
  }
  const act = input.action;
  const actions = actionNames(el);
  const perform = (name) => checkAx($.AXUIElementPerformAction(el, $(name)), name);
  switch (act.type) {
    case 'press': {
      if (actions.indexOf('AXPress') >= 0 && Number($.AXUIElementPerformAction(el, $('AXPress'))) === 0) {
        return { ok: true };
      }
      const f = node.frame;
      if (!f || f.width <= 0 || f.height <= 0) fail('FAILED', 'The element cannot be pressed and has no position to click.');
      activate(input.pid);
      return { ok: true, click: [f.x + f.width / 2, f.y + f.height / 2] };
    }
    case 'focus':
      checkAx($.AXUIElementSetAttributeValue(el, $('AXFocused'), $.NSNumber.numberWithBool(true)), 'focus');
      return { ok: true };
    case 'set_value': {
      const raw = act.value;
      let value = $(raw);
      const numeric = ['AXCheckBox', 'AXRadioButton', 'AXSlider', 'AXIncrementor', 'AXValueIndicator'];
      if (numeric.indexOf(node.role) >= 0 && raw.trim() !== '' && !isNaN(Number(raw))) {
        value = $.NSNumber.numberWithDouble(Number(raw));
      }
      checkAx($.AXUIElementSetAttributeValue(el, $('AXValue'), value), 'setting its value');
      const after = readNode(el);
      return { ok: true, value: after ? after.value : null };
    }
    case 'show_menu':
      perform('AXShowMenu');
      return { ok: true };
    case 'raise': {
      const win = input.path[0] === 'w' ? resolve(app, input.path.slice(0, 2)) : el;
      activate(input.pid);
      checkAx($.AXUIElementPerformAction(win, $('AXRaise')), 'raising its window');
      return { ok: true };
    }
    case 'scroll_into_view':
      perform('AXScrollToVisible');
      return { ok: true };
    case 'perform':
      if (actions.indexOf(act.name) < 0) {
        fail('INVALID_ARGS', 'This element has no action "' + act.name + '". It supports: ' + (actions.join(', ') || 'none') + '.');
      }
      perform(act.name);
      return { ok: true };
    default:
      fail('INVALID_ARGS', 'Unknown action ' + act.type + '.');
  }
}

function run(argv) {
  const input = JSON.parse(argv[0]);
  if (input.mode !== 'action') requireTrust();
  switch (input.mode) {
    case 'tree': return JSON.stringify(tree(input));
    case 'find': return JSON.stringify(find(input));
    case 'action': return JSON.stringify(action(input));
    default: fail('INVALID_ARGS', 'Unknown mode ' + input.mode + '.');
  }
}
