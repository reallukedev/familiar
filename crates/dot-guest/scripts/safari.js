// Safari through its scripting dictionary. Page code is fixed text below; caller values
// (selectors, field values, limits) are passed into it as JSON literals, never spliced.
// Input (argv[0], JSON): { action, ... } mirroring dot_wire::SafariOp, plus budget_ms.
// Output: JSON for the action (see dot_wire::SafariOp).

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

const safari = Application('Safari');

// Build `(fn)(arg1, arg2, ...)` with every argument as a JSON literal.
function call(fnSource, args) {
  return '(' + fnSource + ')(' + args.map((a) => JSON.stringify(a)).join(',') + ')';
}

const READ_PAGE = function (maxChars) {
  function clean(s) {
    return (s || '').replace(/[ \t\f\v ]+/g, ' ').replace(/ *\n[\s]*/g, '\n').trim();
  }
  var root = document.querySelector('article') || document.querySelector('main') ||
    document.querySelector('[role=main]');
  var text = root ? clean(root.innerText) : '';
  if (text.length < 200 && document.body) text = clean(document.body.innerText);
  var truncated = false;
  if (text.length > maxChars) { text = text.slice(0, maxChars); truncated = true; }
  var links = [];
  var seen = {};
  var anchors = document.querySelectorAll('a[href]');
  for (var i = 0; i < anchors.length && links.length < 200; i++) {
    var a = anchors[i];
    var href = a.href;
    if (!href || href.indexOf('javascript:') === 0 || seen[href]) continue;
    seen[href] = true;
    var label = clean(a.innerText || a.getAttribute('aria-label') || a.title || '').slice(0, 200);
    links.push([label, href]);
  }
  return JSON.stringify({ title: document.title, url: location.href, text: text, links: links, truncated: truncated });
}.toString();

const DOM_ACTION = function (sel, act, value) {
  var all;
  try { all = document.querySelectorAll(sel); } catch (e) {
    return JSON.stringify({ error: 'INVALID_ARGS', message: 'Invalid CSS selector: ' + e.message });
  }
  var el = all[0];
  if (!el) return JSON.stringify({ matched: 0 });
  var fire = function (type) { el.dispatchEvent(new Event(type, { bubbles: true })); };
  switch (act) {
    case 'click':
      el.scrollIntoView({ block: 'center' });
      el.click();
      break;
    case 'fill': {
      el.focus();
      if (el.isContentEditable) {
        el.textContent = value;
      } else {
        // React and friends track the native setter, not the property.
        var proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        var desc = Object.getOwnPropertyDescriptor(proto, 'value');
        if (desc && desc.set && el instanceof proto.constructor) desc.set.call(el, value); else el.value = value;
      }
      fire('input');
      fire('change');
      break;
    }
    case 'select': {
      if (!(el instanceof HTMLSelectElement)) {
        return JSON.stringify({ error: 'INVALID_ARGS', message: 'The element is a ' + el.tagName.toLowerCase() + ', not a select.' });
      }
      var found = null;
      for (var i = 0; i < el.options.length; i++) {
        var o = el.options[i];
        if (o.value === value || o.text.trim() === value) { found = o; break; }
      }
      if (!found) return JSON.stringify({ error: 'NOT_FOUND', message: 'The select has no option with value or label "' + value + '".' });
      el.value = found.value;
      fire('input');
      fire('change');
      break;
    }
    case 'submit': {
      var form = el.tagName === 'FORM' ? el : (el.form || el.closest('form'));
      if (!form) return JSON.stringify({ error: 'NOT_FOUND', message: 'The element is not inside a form.' });
      if (form.requestSubmit) form.requestSubmit(); else form.submit();
      break;
    }
    case 'scroll':
      el.scrollIntoView({ block: 'center' });
      break;
    case 'inspect': {
      var r = el.getBoundingClientRect();
      var attrs = {};
      for (var j = 0; j < el.attributes.length && j < 50; j++) {
        attrs[el.attributes[j].name] = String(el.attributes[j].value).slice(0, 500);
      }
      var style = window.getComputedStyle(el);
      return JSON.stringify({
        matched: all.length,
        element: {
          tag: el.tagName.toLowerCase(),
          text: (el.innerText || el.textContent || '').trim().slice(0, 2000),
          value: 'value' in el ? String(el.value).slice(0, 2000) : null,
          attributes: attrs,
          rect: { x: r.x, y: r.y, width: r.width, height: r.height },
          visible: r.width > 0 && r.height > 0 && style.visibility !== 'hidden' && style.display !== 'none',
        },
      });
    }
    default:
      return JSON.stringify({ error: 'INVALID_ARGS', message: 'Unknown DOM action ' + act + '.' });
  }
  return JSON.stringify({ matched: all.length });
}.toString();

const NAVIGATE = function (direction) {
  if (direction === 'back') history.back();
  else if (direction === 'forward') history.forward();
  else location.reload();
  return 'ok';
}.toString();

function requireRunning() {
  if (!safari.running()) fail('NOT_FOUND', 'Safari is not open. Open a URL first.');
}

function windows() {
  return safari.windows();
}

function tabTarget(input) {
  requireRunning();
  const wins = windows();
  if (wins.length === 0) fail('NOT_FOUND', 'Safari has no open windows.');
  const wi = input.window || 1;
  if (wi < 1 || wi > wins.length) fail('NOT_FOUND', 'Safari has no window ' + wi + ' (it has ' + wins.length + ').');
  const w = wins[wi - 1];
  let tab;
  let ti;
  try {
    if (input.tab) {
      const tabs = w.tabs();
      if (input.tab < 1 || input.tab > tabs.length) {
        fail('NOT_FOUND', 'Window ' + wi + ' has no tab ' + input.tab + ' (it has ' + tabs.length + ').');
      }
      tab = tabs[input.tab - 1];
      ti = input.tab;
    } else {
      tab = w.currentTab();
      ti = tab.index();
    }
  } catch (e) {
    if (String(e.message || e).indexOf('NOT_FOUND') >= 0) throw e;
    fail('NOT_FOUND', 'Window ' + wi + ' is not a browser window.');
  }
  return { w, wi, tab, ti };
}

function describe(wi, tab, ti) {
  let current = false;
  if (wi === 1) {
    try { current = windows()[0].currentTab.index() === ti; } catch (e) { current = false; }
  }
  return {
    window: wi,
    tab: ti,
    title: tab.name() || '',
    url: tab.url() || '',
    current: current,
  };
}

function js(tab, code) {
  let out;
  try {
    out = safari.doJavaScript(code, { in: tab });
  } catch (e) {
    const m = String(e.message || e);
    if (/allow javascript from apple events/i.test(m)) throw e;
    fail('FAILED', 'Safari could not run JavaScript in the page: ' + m);
  }
  return out;
}

function pageResult(raw) {
  if (typeof raw !== 'string') fail('FAILED', 'The page returned no result. It may still be loading.');
  const value = JSON.parse(raw);
  if (value && value.error) fail(value.error, value.message);
  return value;
}

function tabs() {
  if (!safari.running()) return [];
  const out = [];
  windows().forEach((w, i) => {
    let names, urls, cur;
    try {
      names = w.tabs.name();
      urls = w.tabs.url();
      cur = w.currentTab.index();
    } catch (e) {
      return;
    }
    for (let j = 0; j < names.length; j++) {
      out.push({ window: i + 1, tab: j + 1, title: names[j] || '', url: urls[j] || '', current: i === 0 && j + 1 === cur });
    }
  });
  return out;
}

function browserWindow() {
  for (const w of windows()) {
    try {
      w.currentTab.index();
      return w;
    } catch (e) {
      // Settings and other non-browser windows have no tabs.
    }
  }
  return null;
}

function open(input) {
  let w = safari.running() ? browserWindow() : null;
  if (!w) {
    safari.Document().make();
    for (let i = 0; i < 30 && !w; i++) { delay(0.1); w = browserWindow(); }
    if (!w) fail('FAILED', 'Safari did not open a window.');
    w.currentTab.url = input.url;
  } else if (input.new_tab) {
    const t = safari.Tab({ url: input.url });
    w.tabs.push(t);
    w.currentTab = t;
  } else {
    w.currentTab.url = input.url;
  }
  w.index = 1;
  safari.activate();
  const tab = w.currentTab();
  return describe(1, tab, tab.index());
}

function activateTab(input) {
  const t = tabTarget(input);
  t.w.currentTab = t.tab;
  t.w.index = 1;
  safari.activate();
  return describe(1, t.w.currentTab(), t.ti);
}

function waitLoaded(tab, budgetMs) {
  const deadline = Date.now() + budgetMs;
  delay(0.3);
  while (Date.now() < deadline) {
    let state = null;
    try { state = js(tab, 'document.readyState'); } catch (e) { state = null; }
    if (state === 'complete') return;
    delay(0.2);
  }
  fail('TIMEOUT', 'The page was still loading when the deadline passed.');
}

function run(argv) {
  const input = JSON.parse(argv[0]);
  switch (input.action) {
    case 'tabs':
      return JSON.stringify(tabs());
    case 'open':
      return JSON.stringify(open(input));
    case 'activate':
      return JSON.stringify(activateTab(input));
    case 'close': {
      const t = tabTarget(input);
      t.tab.close();
      return JSON.stringify({});
    }
    case 'read': {
      const t = tabTarget(input);
      return JSON.stringify(pageResult(js(t.tab, call(READ_PAGE, [input.max_chars]))));
    }
    case 'eval': {
      const t = tabTarget(input);
      // The caller's expression is the one thing that is code by design.
      const code = '(function(){var __v;try{__v=(' + input.script + '\n);}catch(e){return JSON.stringify({error:"FAILED",message:"JavaScript error: "+String(e&&e.message||e)});}' +
        'try{return JSON.stringify({result:__v===undefined?null:__v});}catch(e){return JSON.stringify({result:String(__v)});}})()';
      const raw = js(t.tab, code);
      if (typeof raw !== 'string') {
        fail('INVALID_ARGS', 'Safari could not run that script. It must be a single JavaScript expression; wrap statements as (() => { ...; return value })().');
      }
      return JSON.stringify(pageResult(raw));
    }
    case 'dom': {
      const t = tabTarget(input);
      const act = input.do || {};
      return JSON.stringify(pageResult(js(t.tab, call(DOM_ACTION, [input.selector, act.type, act.value === undefined ? null : act.value]))));
    }
    case 'navigate': {
      const t = tabTarget({});
      js(t.tab, call(NAVIGATE, [input.direction]));
      delay(0.3);
      return JSON.stringify(describe(t.wi, t.tab, t.ti));
    }
    case 'wait_loaded': {
      const t = tabTarget({});
      waitLoaded(t.tab, input.budget_ms);
      return JSON.stringify(describe(t.wi, t.tab, t.ti));
    }
    default:
      fail('INVALID_ARGS', 'Unknown Safari action ' + input.action + '.');
  }
}
