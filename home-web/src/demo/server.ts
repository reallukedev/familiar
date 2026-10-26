// The demo backend: every endpoint in docs/home/API.md, answered from synthetic
// in-memory state. Development and tests only; never part of a production build.
import type { Activity, AdminInstance, AgentChannel, AppEntry, Approval, Channel, Instance, Release, SessionInfo, Snapshot, UpdatesStatus, User } from '../lib/types';
import * as data from './data';
import { registerExtras } from './extras';
import { registerReview, chooseMode } from './review';
import { onAppServerMode, registerAppServers } from './appservers';
import { dropAll, emit } from './events';
import { checkResetKey, lockedGuard, newRecoveryKey, registerEncryption, resetInviteInfo, unlock } from './encryption';
import type { DemoState } from './state';

type Result = { status: number; body?: unknown; blob?: Blob };
type Ctx = { state: DemoState; method: string; path: string; query: URLSearchParams; body: Record<string, unknown>; headers: Headers };
type Handler = (ctx: Ctx, ...params: string[]) => Result | Promise<Result>;

const json = (body: unknown, status = 200): Result => ({ status, body });
const fail = (status: number, code: string, message: string, extra: Record<string, unknown> = {}, field?: string): Result => ({
  status,
  body: { error: { code, message, ...(field ? { field } : {}) }, ...extra },
});
const notFound = () => fail(404, 'not_found', 'That Mac doesn’t exist, or it isn’t yours.');

function me(state: DemoState): data.DemoUser | null {
  return state.users.find((user) => user.id === state.signedIn) ?? null;
}
function publicUser(user: data.DemoUser): User {
  return { id: user.id, username: user.username, display_name: user.display_name, role: user.role, totp_enabled: user.totp_enabled, created_at: user.created_at };
}
export function sessionInfo(state: DemoState): SessionInfo {
  const user = me(state);
  return {
    user: user ? publicUser(user) : null, csrf: user ? state.csrf : null, setup_required: state.setupRequired, require_totp: state.requireTotp, version: state.updates.current.version, public_url: data.DEMO_ORIGIN, vm_url: 'http://10.0.2.2:8443',
    locked: !!user && state.locked, recovery_key_needed: !!user && state.recoveryKeyNeeded,
  };
}
function mine(state: DemoState, id: string): Instance | null {
  const user = me(state);
  const instance = state.instances.find((item) => item.id === id);
  return instance && user && state.owner[id] === user.id ? instance : null;
}
function sensitive(state: DemoState): Result | null {
  return Date.now() < state.confirmedUntil ? null : fail(403, 'reauth_required', 'Confirm your password to continue.');
}
function update(_state: DemoState, instance: Instance, patch: Partial<Instance>) {
  Object.assign(instance, patch);
  emit(instance.id, 'instance', instance);
}

const timers = new Map<string, ReturnType<typeof setTimeout>[]>();
function schedule(id: string, steps: [number, () => void][]) {
  timers.get(id)?.forEach(clearTimeout);
  timers.set(id, steps.map(([ms, fn]) => setTimeout(fn, ms)));
}

/** Walk a Mac through its boot states, as the supervisor would report them. */
export function boot(state: DemoState, instance: Instance, fast: boolean) {
  const k = fast ? 0.35 : 1;
  update(state, instance, { state: 'starting', state_detail: 'Turning on the virtual machine.' });
  schedule(instance.id, [
    [1500 * k, () => update(state, instance, { state: 'booting', state_detail: 'Waiting for macOS to finish starting.' })],
    [4200 * k, () => update(state, instance, { state: 'booting', state_detail: 'Connecting to the Familiar agent inside the Mac.' })],
    [
      6000 * k,
      () =>
        update(state, instance, {
          state: 'ready',
          state_detail: null,
          booted_at: new Date().toISOString(),
          guest: instance.guest ?? data.instance(instance.id, instance.name, 'ready').guest,
        }),
    ],
  ]);
}

let activitySeq = 900_000;
export function newActivity(state: DemoState, id: string, outcome: Activity['outcome'] = 'ok'): Activity {
  const base = data.activity(activitySeq, 1, outcome === 'error' ? 1 : 0)[0]!;
  const item: Activity = { ...base, id: String(++activitySeq), at: new Date().toISOString(), outcome, error_code: outcome === 'error' ? 'TIMEOUT' : null };
  state.activity[id] = [item, ...(state.activity[id] ?? [])];
  const instance = state.instances.find((entry) => entry.id === id);
  if (instance) {
    instance.counts.calls_24h += 1;
    instance.last_active_at = item.at;
    emit(id, 'instance', instance);
  }
  emit(id, 'activity', item);
  return item;
}

let approvalSeq = 50;
export function newApproval(state: DemoState, id: string): Approval {
  const item = { ...data.approval(id, approvalSeq++, 0), id: `apr-live-${approvalSeq}` };
  state.approvals[id] = [item, ...(state.approvals[id] ?? [])];
  const instance = state.instances.find((entry) => entry.id === id);
  if (instance) {
    instance.counts.pending_approvals += 1;
    emit(id, 'instance', instance);
  }
  emit(id, 'approval', item);
  return item;
}

const routes: [string, RegExp, Handler][] = [];
const on = (method: string, pattern: string, handler: Handler) => routes.push([method, new RegExp(`^${pattern.replace(/:[a-z]+/g, '([^/]+)')}$`), handler]);

// ---- Session and sign-in ----
on('GET', '/api/session', ({ state }) => json(sessionInfo(state)));
on('POST', '/api/setup', ({ state, body }) => {
  if (!state.setupRequired) return fail(409, 'conflict', 'Familiar is already set up. Sign in instead.');
  if (body.token !== 'demo-setup') return fail(403, 'forbidden', 'This setup link is no longer valid. Restart the server to print a new one.');
  if (String(body.password ?? '').length < 10) return fail(400, 'invalid', 'Use at least 10 characters.', {}, 'password');
  const user: data.DemoUser = { id: 'u-new', username: String(body.username), display_name: String(body.display_name), role: 'admin', totp_enabled: false, created_at: new Date().toISOString(), disabled: false, max_instances: 4, instance_count: 0, last_seen_at: null, password: String(body.password) };
  state.users = [user];
  state.setupRequired = false;
  state.signedIn = user.id;
  // A new account's key is made with its password; its recovery key is shown once.
  unlock(state);
  state.recoveryKeyNeeded = true;
  return json({ ...sessionInfo(state), recovery_key: newRecoveryKey() });
});
on('POST', '/api/login', ({ state, body }) => {
  const user = state.users.find((item) => item.username === body.username);
  if (!user || user.password !== body.password) return fail(401, 'unauthenticated', 'That username and password don’t match.');
  if (user.disabled) return fail(403, 'forbidden', 'This account is turned off. Ask the person who runs this Familiar.');
  if (user.totp_enabled) {
    state.challenge = { id: 'challenge-1', user: user.id };
    return fail(401, 'totp_required', 'Enter the code from your authenticator app.', { challenge: 'challenge-1' });
  }
  state.signedIn = user.id;
  unlock(state);
  return json(sessionInfo(state));
});
on('POST', '/api/login/totp', ({ state, body }) => {
  if (!state.challenge || body.challenge !== state.challenge.id) return fail(401, 'unauthenticated', 'That sign-in took too long. Enter your password again.', { restart: 'true' });
  if (body.code !== data.DEMO_TOTP) return fail(400, 'invalid', 'That code isn’t right. Codes change every 30 seconds; try the newest one.', {}, 'code');
  const recoveryKey = state.challenge.recoveryKey;
  state.signedIn = state.challenge.user;
  state.challenge = null;
  unlock(state);
  return json(recoveryKey ? { ...sessionInfo(state), recovery_key: recoveryKey } : sessionInfo(state));
});
on('POST', '/api/logout', ({ state }) => {
  state.signedIn = null;
  return json({});
});
on('GET', '/api/invites/:code', ({ state }, code) => {
  if (code === 'demo-join') return json({ role: 'member', expires_at: new Date(Date.now() + 3 * 86_400_000).toISOString() });
  if (code === 'demo-reset') return json(resetInviteInfo('sam'));
  if (code === 'demo-reset-2fa') return json(resetInviteInfo('jo.park'));
  void state;
  return fail(404, 'not_found', 'This invite expired or was already used.');
});
on('POST', '/api/invites/:code/accept', ({ state, body }, code) => {
  if (code !== 'demo-join' && code !== 'demo-reset' && code !== 'demo-reset-2fa') return fail(404, 'not_found', 'This invite expired or was already used.');
  if (code === 'demo-reset-2fa') {
    // jo.park has two-factor: the reset answers with a code challenge, which holds the key.
    const refused = checkResetKey(body, fail);
    if (refused) return refused;
    const recoveryKey = body.discard_encrypted_macs === true ? newRecoveryKey() : undefined;
    if (recoveryKey) state.recoveryKeyNeeded = true;
    state.challenge = { id: 'challenge-reset', user: 'u-jo', recoveryKey };
    return fail(401, 'totp_required', 'Enter the 6-digit code from your authenticator app.', { challenge: 'challenge-reset' });
  }
  if (code === 'demo-reset') {
    const refused = checkResetKey(body, fail);
    if (refused) return refused;
    state.signedIn = 'u-sam';
    unlock(state);
    if (body.discard_encrypted_macs === true) {
      // A new key replaces the lost one; the old encrypted Macs are gone.
      state.recoveryKeyNeeded = true;
      return json({ ...sessionInfo(state), recovery_key: newRecoveryKey() });
    }
    return json(sessionInfo(state));
  }
  if (state.users.some((user) => user.username === body.username)) return fail(409, 'conflict', 'That username is taken. Try another.', {}, 'username');
  const user: data.DemoUser = { id: `u-${String(body.username)}`, username: String(body.username), display_name: String(body.display_name), role: 'member', totp_enabled: false, created_at: new Date().toISOString(), disabled: false, max_instances: 2, instance_count: 0, last_seen_at: null, password: String(body.password) };
  state.users.push(user);
  state.signedIn = user.id;
  unlock(state);
  state.recoveryKeyNeeded = true;
  return json({ ...sessionInfo(state), recovery_key: newRecoveryKey() });
});

// ---- Account ----
on('GET', '/api/account', ({ state }) => json({ user: publicUser(me(state)!), sessions: state.sessions }));
on('PATCH', '/api/account', ({ state, body }) => {
  const user = me(state)!;
  user.display_name = String(body.display_name ?? user.display_name);
  return json(publicUser(user));
});
on('POST', '/api/account/password', ({ state, body }) => {
  const gate = sensitive(state);
  if (gate) return gate;
  const user = me(state)!;
  if (body.current !== user.password) return fail(400, 'invalid', 'Your current password isn’t right.', {}, 'current');
  user.password = String(body.password);
  state.sessions = state.sessions.filter((item) => item.current);
  return json({});
});
on('POST', '/api/account/totp', ({ state }) => {
  // Changing an existing code asks for the password again, as the server does.
  if (me(state)!.totp_enabled) {
    const gate = sensitive(state);
    if (gate) return gate;
  }
  state.pendingSecret = 'JBSWY3DPEHPK3PXPDEMOSYNTHETIC';
  const user = me(state)!;
  return json({ secret: state.pendingSecret, otpauth_url: `otpauth://totp/Familiar:${user.username}?secret=${state.pendingSecret}&issuer=Familiar` });
});
on('POST', '/api/account/totp/confirm', ({ state, body }) => {
  if (body.code !== data.DEMO_TOTP) return fail(400, 'invalid', 'That code isn’t right. Try the newest one from your app.', {}, 'code');
  const user = me(state)!;
  user.totp_enabled = true;
  return json(publicUser(user));
});
on('DELETE', '/api/account/sessions/:id', ({ state }, id) => {
  state.sessions = state.sessions.filter((item) => item.id !== id);
  return json({});
});
on('POST', '/api/account/confirm', ({ state, body }) => {
  const user = me(state)!;
  if (body.password !== user.password) return fail(400, 'invalid', 'That password isn’t right.', {}, 'password');
  if (user.totp_enabled && body.code !== data.DEMO_TOTP) return fail(400, 'invalid', 'That code isn’t right.', {}, 'code');
  state.confirmedUntil = Date.now() + 10 * 60_000;
  // Confirming the password also unlocks encrypted Macs after a restart.
  unlock(state);
  return json({ until: new Date(state.confirmedUntil).toISOString() });
});
on('GET', '/api/account/security', ({ state }) => json(state.security));

// ---- Instances ----
on('GET', '/api/instances', ({ state }) => {
  const user = me(state)!;
  return json(state.instances.filter((item) => state.owner[item.id] === user.id && !state.hidden.has(item.id)));
});
on('POST', '/api/instances', ({ state, body }) => {
  const user = me(state)!;
  const count = Object.values(state.owner).filter((owner) => owner === user.id).length;
  if (count >= user.max_instances) return fail(409, 'budget', `You can have up to ${user.max_instances} Macs. Delete one, or ask an admin for a higher limit.`);
  const id = `mac-${Date.now().toString(36)}`;
  const remote = body.driver === 'remote';
  const instance = data.instance(id, String(body.name), 'stopped', {
    driver: remote ? 'remote' : 'qemu',
    remote_host: remote ? String(body.remote_host) : null,
    cpus: Number(body.cpus ?? 4),
    memory_mb: Number(body.memory_mb ?? 8192),
    disk_gb: Number(body.disk_gb ?? 128),
    guest: null,
    last_active_at: null,
    created_at: new Date().toISOString(),
    counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 },
    // New Macs on this server are encrypted from the start.
    encrypted: !remote,
    locked: !remote && state.locked,
    can_encrypt: false,
  });
  state.instances.push(instance);
  state.owner[id] = user.id;
  state.activity[id] = [];
  state.approvals[id] = [];
  state.apps[id] = [];
  state.permissions[id] = data.permissions();
  chooseMode(state, id, body.approval_mode);
  state.snapshots[id] = [];
  state.connections[id] = { mcp_url: instance.mcp_url, grants: [], tokens: [] };
  user.instance_count += 1;
  return json({ instance, ...(remote ? { pairing: `dotpair:v1:${id}:c3ludGhldGljLWRlbW8tc2VjcmV0` } : {}) });
});
on('GET', '/api/instances/:id', ({ state }, id) => mine(state, id) ? json(mine(state, id)) : notFound());
on('PATCH', '/api/instances/:id', ({ state, body }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  if ('snapshot_keep_days' in body) {
    const days = Number(body.snapshot_keep_days);
    if (!Number.isInteger(days) || days < 0 || days > 3650) return fail(400, 'invalid', 'Keep snapshots for 1 to 3650 days, or 0 for forever.', {}, 'snapshot_keep_days');
  }
  if ('snapshot_max_gb' in body) {
    const gb = Number(body.snapshot_max_gb);
    if (!Number.isInteger(gb) || gb < 0 || gb > 100_000) return fail(400, 'invalid', 'Choose up to 100000 GB, or 0 for no limit.', {}, 'snapshot_max_gb');
  }
  const allowed = ['name', 'paused', 'cpus', 'memory_mb', 'sleep_after_minutes', 'hibernate_after_minutes', 'snapshot_keep_days', 'snapshot_max_gb'] as const;
  const patch: Partial<Instance> = {};
  for (const key of allowed) if (key in body) (patch as Record<string, unknown>)[key] = body[key];
  if ('agent_channel' in body && instance.agent) {
    const channel = body.agent_channel as AgentChannel;
    if (!['default', 'stable', 'nightly', 'off'].includes(channel)) return fail(400, 'invalid', 'Choose Stable, Nightly, Off or the server setting.', {}, 'agent_channel');
    patch.agent = resolveAgent(state, { ...instance.agent, channel });
  }
  update(state, instance, patch);
  return json(instance);
});
on('DELETE', '/api/instances/:id', ({ state, body }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const gate = sensitive(state);
  if (gate) return gate;
  if (body.confirm !== instance.name) return fail(400, 'invalid', 'Type the Mac’s name exactly to delete it.', {}, 'confirm');
  state.instances = state.instances.filter((item) => item.id !== id);
  delete state.owner[id];
  return json({});
});
on('POST', '/api/instances/:id/power', ({ state, body }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const fast = typeof navigator !== 'undefined' && navigator.webdriver;
  const action = String(body.action);
  if (action === 'start' || action === 'restart') boot(state, instance, fast);
  else if (action === 'wake') {
    update(state, instance, { state: 'starting', state_detail: instance.state === 'hibernated' ? 'Restoring memory from disk.' : 'Waking up.' });
    schedule(id, [[fast ? 400 : 1500, () => update(state, instance, { state: 'ready', state_detail: null })]]);
  } else if (action === 'sleep') update(state, instance, { state: 'sleeping', state_detail: null });
  else if (action === 'stop' || action === 'force_stop') {
    update(state, instance, { state: 'stopping', state_detail: null });
    schedule(id, [[fast ? 300 : 2000, () => update(state, instance, { state: 'stopped', state_detail: null, booted_at: null })]]);
  }
  return json(instance);
});
on('POST', '/api/instances/:id/agent/update', ({ state }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const agent = instance.agent;
  if (!agent || !agent.can_update) return fail(409, 'conflict', 'This Mac’s agent is too old to update itself. Reinstall it instead.');
  if (instance.state !== 'ready' && instance.state !== 'degraded') return fail(409, 'conflict', `Start ${instance.name} first. Its agent updates while the Mac is running.`);
  if (!agent.available) return fail(409, 'conflict', 'This Mac’s agent is already up to date.');
  if (agent.updating) return json(instance);
  const target = agent.available;
  const from = agent.version;
  update(state, instance, { agent: { ...agent, updating: true } });
  const fast = typeof navigator !== 'undefined' && navigator.webdriver;
  schedule(`agent-${id}`, [
    [fast ? 900 : 2600, () => {
      update(state, instance, { agent: { ...instance.agent!, version: target, available: null, updating: false } });
      record(state, { target: 'agent', target_name: instance.name, from_version: from, to_version: target, outcome: 'installed', detail: null });
    }],
  ]);
  return json(instance);
});
on('GET', '/api/instances/:id/thumbnail', async ({ state }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  if (instance.state !== 'ready' && instance.state !== 'degraded') return { status: 204 };
  const { thumbnail } = await import('./screen');
  return { status: 200, blob: await thumbnail(id, instance.name) };
});

// ---- Permissions and approvals ----
registerReview(on, { json, fail, mine });
registerAppServers(on, { json, fail, mine });
on('GET', '/api/instances/:id/permissions', ({ state }, id) => (mine(state, id) ? json(state.permissions[id]) : notFound()));
on('PUT', '/api/instances/:id/permissions/:group', ({ state, body }, id, group) => {
  const target = state.permissions[id]?.find((item) => item.id === decodeURIComponent(group));
  if (!mine(state, id) || !target) return notFound();
  target.mode = body.mode as typeof target.mode;
  onAppServerMode(target);
  return json(target);
});
on('GET', '/api/instances/:id/approvals', ({ state }, id) => (mine(state, id) ? json(state.approvals[id] ?? []) : notFound()));
on('POST', '/api/instances/:id/approvals/:approval', ({ state, body }, id, approvalId) => {
  const instance = mine(state, id);
  const target = state.approvals[id]?.find((item) => item.id === approvalId);
  if (!instance || !target) return notFound();
  if (target.status !== 'pending') return fail(409, 'conflict', 'This request was already answered or has expired.');
  target.status = body.decision === 'approve' ? 'approved' : 'denied';
  instance.counts.pending_approvals = Math.max(0, instance.counts.pending_approvals - 1);
  emit(id, 'approval', target);
  emit(id, 'instance', instance);
  if (target.status === 'approved') setTimeout(() => newActivity(state, id), 600);
  return json(target);
});

// ---- Activity ----
on('GET', '/api/instances/:id/activity', ({ state, query }, id) => {
  if (!mine(state, id)) return notFound();
  let items = state.activity[id] ?? [];
  if (query.get('outcome') === 'error') items = items.filter((item) => item.outcome === 'error');
  const before = query.get('before');
  if (before) items = items.filter((item) => Number(item.id) < Number(before));
  const limit = Number(query.get('limit') ?? 50);
  const page = items.slice(0, limit);
  return json({ items: page, next: items.length > limit ? page.at(-1)!.id : null });
});

// ---- Clipboard (the screen's bridge to the Mac) ----
const macClipboards = new Map<string, string>();
on('GET', '/api/instances/:id/clipboard', ({ state }, id) => (mine(state, id) ? json({ text: macClipboards.get(id) ?? null }) : notFound()));
on('PUT', '/api/instances/:id/clipboard', ({ state, body }, id) => {
  if (!mine(state, id)) return notFound();
  macClipboards.set(id, String(body.text ?? ''));
  return { status: 204 };
});

// ---- Apps ----
on('GET', '/api/instances/:id/apps', ({ state }, id) => (mine(state, id) ? json(state.apps[id] ?? []) : notFound()));
on('PUT', '/api/instances/:id/apps/:app', ({ state, body }, id, appId) => {
  const app = state.apps[id]?.find((item) => item.id === decodeURIComponent(appId));
  if (!mine(state, id) || !app) return notFound();
  app.enabled = !!body.enabled;
  return json(app);
});
on('POST', '/api/instances/:id/apps/install', ({ state, body }, id) => {
  if (!mine(state, id)) return notFound();
  const source = body.source as { type: string; url?: string; id?: string; cask?: string };
  if (source.type === 'brew' && source.cask === 'slow-app') return json({ installing: true }, 202);
  const name = source.type === 'brew' ? source.cask!.replace(/-/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase()) : source.type === 'app_store' ? `App Store app ${source.id}` : decodeURIComponent(new URL(source.url!).pathname.split('/').pop() ?? 'App').replace(/\.(dmg|zip|pkg)$/i, '');
  const app: AppEntry = { id: `installed.${Date.now()}`, name, bundle_id: null, installed: true, running: false, version: null, scriptable: false, enabled: false, kind: 'ui_only', tools: [], icon: null };
  state.apps[id] = [...(state.apps[id] ?? []), app];
  return json(app);
});
on('POST', '/api/instances/:id/apps/custom', ({ state, body }, id) => {
  if (!mine(state, id)) return notFound();
  const manifest = String(body.manifest ?? '');
  const idMatch = manifest.match(/^id\s*=\s*"([^"]+)"/m)?.[1];
  if (manifest.includes('osascript -e')) return fail(400, 'invalid', 'Scripts can’t start other interpreters. Write the logic in the script itself.', {}, 'tools[0].script');
  const name = manifest.match(/^name\s*=\s*"([^"]+)"/m)?.[1] ?? idMatch ?? 'Custom';
  const tools = [...manifest.matchAll(/^name\s*=\s*"([a-z0-9_]+)"\s*\ntitle\s*=\s*"([^"]+)"[\s\S]*?risk\s*=\s*"(\w+)"/gm)].map((m) => ({ name: m[1]!, title: m[2]!, risk: m[3] as 'read' }));
  const app: AppEntry = { id: idMatch ?? 'custom', name, bundle_id: manifest.match(/^bundle_id\s*=\s*"([^"]+)"/m)?.[1] ?? null, installed: true, running: false, version: null, scriptable: true, enabled: true, kind: 'custom', tools, icon: null };
  state.apps[id] = [...(state.apps[id] ?? []).filter((item) => item.id !== app.id), app];
  return json(app);
});
on('DELETE', '/api/instances/:id/apps/custom/:app', ({ state }, id, appId) => {
  state.apps[id] = (state.apps[id] ?? []).filter((item) => item.id !== decodeURIComponent(appId));
  return json({});
});

// ---- Snapshots ----
const SNAP_GB = 1_000_000_000;

/** When retention deletes it, as the server reports it. */
function withExpiry(instance: Instance, snapshot: Snapshot): Snapshot {
  const days = instance.snapshot_keep_days ?? 7;
  const expires_at = snapshot.kept || days <= 0 ? null : new Date(Date.parse(snapshot.created_at) + days * 86_400_000).toISOString();
  return { ...snapshot, expires_at };
}

/** Oldest unkept snapshots to drop until `list` fits in `cap` bytes; none when kept ones alone pass it. */
function overCap(list: Snapshot[], cap: number, protect?: string): Set<string> {
  const drop = new Set<string>();
  const size = (s: Snapshot) => s.size_bytes ?? 0;
  let total = list.reduce((sum, s) => sum + size(s), 0);
  const kept = list.filter((s) => s.kept).reduce((sum, s) => sum + size(s), 0);
  if (cap <= 0 || total <= cap || kept > cap) return drop;
  for (const s of list.filter((item) => !item.kept && item.id !== protect && size(item) > 0).toSorted((a, b) => a.created_at.localeCompare(b.created_at))) {
    if (total <= cap) break;
    total -= size(s);
    drop.add(s.id);
  }
  return drop;
}

/** The server's sweeper: retention, then this Mac's cap, then the server-wide cap. */
function sweepSnapshots(state: DemoState, instance: Instance, protect?: string) {
  const now = Date.now();
  let list = (state.snapshots[instance.id] ?? []).map((s) => withExpiry(instance, s)).filter((s) => !s.expires_at || Date.parse(s.expires_at) > now || s.id === protect);
  const mac = overCap(list, (instance.snapshot_max_gb ?? 0) * SNAP_GB, protect);
  list = list.filter((s) => !mac.has(s.id));
  state.snapshots[instance.id] = list;
  const total = state.extras.defaults.snapshot_total_gb ?? 0;
  if (total > 0) {
    const all = Object.values(state.snapshots).flat();
    const server = overCap(all, total * SNAP_GB, protect);
    if (server.size) for (const [id, items] of Object.entries(state.snapshots)) state.snapshots[id] = items.filter((s) => !server.has(s.id));
  }
}

on('GET', '/api/instances/:id/snapshots', ({ state }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  sweepSnapshots(state, instance);
  return json((state.snapshots[id] ?? []).map((s) => withExpiry(instance, s)));
});
on('POST', '/api/instances/:id/snapshots', ({ state, body }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const snapshot: Snapshot = withExpiry(instance, { id: `${id}-snap-${Date.now()}`, name: String(body.name), created_at: new Date().toISOString(), live: instance.state === 'ready', size_bytes: 3_200_000_000, created_by: 'owner', kept: false, expires_at: null });
  state.snapshots[id] = [snapshot, ...(state.snapshots[id] ?? [])];
  sweepSnapshots(state, instance, snapshot.id);
  return json(snapshot);
});
on('PUT', '/api/instances/:id/snapshots/:sid', ({ state, body }, id, sid) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const snapshot = (state.snapshots[id] ?? []).find((item) => item.id === sid);
  if (!snapshot) return fail(404, 'not_found', 'That Snapshot doesn’t exist or isn’t yours.');
  snapshot.kept = body.kept === true;
  return json(withExpiry(instance, snapshot));
});
on('GET', '/api/instances/:id/snapshot-usage', ({ state }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  const list = state.snapshots[id] ?? [];
  const used = list.reduce((sum, s) => sum + (s.size_bytes ?? 0), 0);
  const kept = list.filter((s) => s.kept).reduce((sum, s) => sum + (s.size_bytes ?? 0), 0);
  const cap = instance.snapshot_max_gb ?? 0;
  const limit = cap > 0 ? cap * SNAP_GB : null;
  return json({ used_bytes: used, kept_bytes: kept, limit_bytes: limit, kept_over_limit: limit !== null && kept > limit });
});
on('POST', '/api/instances/:id/snapshots/:sid/restore', ({ state }, id) => {
  const instance = mine(state, id);
  if (!instance) return notFound();
  boot(state, instance, typeof navigator !== 'undefined' && navigator.webdriver);
  return json(instance);
});
on('DELETE', '/api/instances/:id/snapshots/:sid', ({ state }, id, sid) => {
  state.snapshots[id] = (state.snapshots[id] ?? []).filter((item) => item.id !== sid);
  return json({});
});

// ---- Connections ----
on('GET', '/api/instances/:id/connections', ({ state }, id) => (mine(state, id) ? json(state.connections[id]) : notFound()));
on('POST', '/api/instances/:id/tokens', ({ state, body }, id) => {
  if (!mine(state, id)) return notFound();
  const gate = sensitive(state);
  if (gate) return gate;
  const days = body.expires_in_days ? Number(body.expires_in_days) : null;
  const token = { id: `t-${Date.now()}`, name: String(body.name), prefix: 'dot_pat_Ab3x', created_at: new Date().toISOString(), expires_at: days ? new Date(Date.now() + days * 86_400_000).toISOString() : null, last_used_at: null };
  state.connections[id]!.tokens = [token, ...state.connections[id]!.tokens];
  return json({ token, secret: 'dot_pat_Ab3xSYNTHETICdemoTOKENnotREAL0000' });
});
on('DELETE', '/api/instances/:id/tokens/:tid', ({ state }, id, tid) => {
  state.connections[id]!.tokens = state.connections[id]!.tokens.filter((item) => item.id !== tid);
  return json({});
});
on('DELETE', '/api/instances/:id/grants/:gid', ({ state }, id, gid) => {
  state.connections[id]!.grants = state.connections[id]!.grants.filter((item) => item.id !== gid);
  return json({});
});

// ---- OAuth consent ----
on('GET', '/api/oauth/requests/:id', ({ state }, id) => (state.oauth[id] ? json(state.oauth[id]) : fail(404, 'not_found', 'This request expired. Start connecting again from your assistant.')));
on('POST', '/api/oauth/requests/:id', ({ state, body }, id) => {
  const request = state.oauth[id];
  if (!request) return fail(404, 'not_found', 'This request expired. Start connecting again from your assistant.');
  delete state.oauth[id];
  return json({ redirect: `${location.origin}/demo-oauth-return?client=${encodeURIComponent(request.client_name)}&approved=${body.approve ? 1 : 0}` });
});

// ---- Admin ----
function admin(state: DemoState): Result | null {
  return me(state)?.role === 'admin' ? null : fail(403, 'forbidden', 'Only administrators can do that.');
}
on('GET', '/api/admin/overview', ({ state }) => admin(state) ?? json({ ...data.overview(state.overviewFailing), version: state.updates.current.version, require_totp: state.requireTotp }));
on('GET', '/api/admin/users', ({ state }) => admin(state) ?? json(state.users.map(({ password: _p, ...user }) => user)));
on('PATCH', '/api/admin/users/:id', ({ state, body }, id) => {
  const gate = admin(state) ?? sensitive(state);
  if (gate) return gate;
  const user = state.users.find((item) => item.id === id);
  if (!user) return notFound();
  if ('disabled' in body) user.disabled = !!body.disabled;
  if ('role' in body) user.role = body.role as typeof user.role;
  if ('max_instances' in body) user.max_instances = Number(body.max_instances);
  const { password: _p, ...rest } = user;
  return json(rest);
});
on('DELETE', '/api/admin/users/:id', ({ state, body }, id) => {
  const gate = admin(state) ?? sensitive(state);
  if (gate) return gate;
  const user = state.users.find((item) => item.id === id);
  if (!user || body.confirm !== user.username) return fail(400, 'invalid', 'Type the username exactly to delete this person.', {}, 'confirm');
  state.users = state.users.filter((item) => item.id !== id);
  return json({});
});
on('POST', '/api/admin/users/:id/reset', ({ state }) => admin(state) ?? json({ invite_url: `${data.DEMO_ORIGIN}/invite/demo-reset` }));
on('GET', '/api/admin/instances', ({ state }) => {
  const gate = admin(state);
  if (gate) return gate;
  const owners = ['alex', 'sam', 'jo.park'];
  const list: AdminInstance[] = state.instances.filter((item) => !state.hidden.has(item.id)).map((item, i) => ({ id: item.id, name: item.name, owner: state.users.find((user) => user.id === state.owner[item.id])?.username ?? owners[i % 3]!, state: item.state, memory_mb: item.memory_mb, cpus: item.cpus, last_active_at: item.last_active_at }));
  list.push({ id: 'jo-mac', name: 'Jo’s Mac', owner: 'jo.park', state: 'ready', memory_mb: 8192, cpus: 4, last_active_at: new Date(Date.now() - 600_000).toISOString() });
  return json(list);
});
on('POST', '/api/admin/instances/:id/stop', ({ state }) => admin(state) ?? json({}));
on('GET', '/api/admin/invites', ({ state }) => admin(state) ?? json(state.invites));
on('POST', '/api/admin/invites', ({ state, body }) => {
  const gate = admin(state);
  if (gate) return gate;
  const invite = { id: `inv-${Date.now()}`, role: body.role as 'member', created_at: new Date().toISOString(), expires_at: new Date(Date.now() + Number(body.expires_in_hours ?? 72) * 3_600_000).toISOString(), used: false };
  state.invites = [invite, ...state.invites];
  return json({ invite, url: `${data.DEMO_ORIGIN}/invite/demo-join` });
});
on('DELETE', '/api/admin/invites/:id', ({ state }, id) => {
  state.invites = state.invites.filter((item) => item.id !== id);
  return json({});
});
on('GET', '/api/admin/images', ({ state }) => admin(state) ?? json(state.image));
on('POST', '/api/admin/images/builder', ({ state }) => {
  state.image = data.image('downloading');
  const tick = (progress: number, phase: 'downloading' | 'installing') => {
    if (!state.image.builder) return;
    state.image = { ...state.image, builder: { ...state.image.builder, phase, progress } };
    emit('builder', 'image', state.image);
  };
  schedule('builder', [
    [1500, () => tick(0.7, 'downloading')],
    [3000, () => tick(0.95, 'downloading')],
    [4500, () => tick(0.1, 'installing')],
  ]);
  if (!state.instances.some((item) => item.id === 'builder')) {
    state.instances.push(data.instance('builder', 'macOS Installer', 'ready', { counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 } }));
    state.owner.builder = state.signedIn!;
    state.hidden.add('builder');
    state.activity.builder = [];
  }
  return json(state.image);
});
on('POST', '/api/admin/images/promote', ({ state }) => {
  state.image = { ...data.image(null), base: { present: true, size_bytes: 25_100_000_000, created_at: new Date().toISOString(), macos: 'macOS Tahoe 26' } };
  return json(state.image);
});
on('DELETE', '/api/admin/images/builder', ({ state }) => {
  state.image = data.image(null);
  return json(state.image);
});
on('PUT', '/api/admin/settings', ({ state, body }) => {
  const gate = admin(state);
  if (gate) return gate;
  if (body.require_totp === true && !me(state)!.totp_enabled) return fail(400, 'invalid', 'Turn on two-factor authentication for your own account first.', {}, 'require_totp');
  if ('require_totp' in body) state.requireTotp = !!body.require_totp;
  const overview = data.overview(state.overviewFailing);
  if ('memory_budget_mb' in body) overview.budget.memory_mb = Number(body.memory_budget_mb);
  if ('max_running' in body) overview.budget.max_running = Number(body.max_running);
  return json({ ...overview, require_totp: state.requireTotp });
});

// ---- Updates ----
function latest(channel: Channel): Release {
  return channel === 'stable' ? data.STABLE.at(-1)! : data.nightlies().at(-1)!;
}
/** What the server's channel offers, the way the real check would answer. */
function availableFor(status: UpdatesStatus): UpdatesStatus['available'] {
  if (status.settings.channel === 'off') return null;
  const release = latest(status.settings.channel);
  if (release.version === status.current.version) return { ...release, newer: false };
  // Nightly builds sort after the stable release they lead to; stable never passes a newer nightly.
  const newer = status.settings.channel === 'nightly' || status.current.channel !== 'nightly';
  return { ...release, newer };
}
function resolveAgent(state: DemoState, agent: NonNullable<Instance['agent']>): NonNullable<Instance['agent']> {
  const effective = agent.channel === 'default' ? state.updates.settings.guest_default_channel : agent.channel;
  const target = effective === 'off' ? null : latest(effective).version;
  return { ...agent, effective_channel: effective, available: target && target !== agent.version ? target : null };
}
let eventSeq = 100;
function record(state: DemoState, event: Omit<UpdatesStatus['history'][number], 'id' | 'at'>) {
  state.updates = { ...state.updates, history: [{ ...event, id: `ue-live-${++eventSeq}`, at: new Date().toISOString() }, ...state.updates.history].slice(0, 50) };
  emit('@admin', 'updates', state.updates);
}
function setUpdates(state: DemoState, patch: Partial<UpdatesStatus>) {
  state.updates = { ...state.updates, ...patch };
  emit('@admin', 'updates', state.updates);
}

/**
 * Download, install, restart: the server goes away for a few seconds and comes
 * back on `target`. Every request fails meanwhile, as it would for real.
 */
export function runInstall(state: DemoState, target: Release, outcome: 'installed' | 'rolled_back') {
  const fast = typeof navigator !== 'undefined' && navigator.webdriver;
  const k = fast ? 0.4 : 1;
  const from = state.updates.current;
  const rollback = outcome === 'rolled_back';
  setUpdates(state, { state: rollback ? 'restarting' : 'downloading', detail: rollback ? null : `Downloading Familiar ${target.version}.` });
  const steps: [number, () => void][] = [];
  if (!rollback) {
    steps.push([1400 * k, () => setUpdates(state, { state: 'downloading', detail: 'Checking the download’s signature.' })]);
    steps.push([2400 * k, () => setUpdates(state, { state: 'installing', detail: 'Installing and checking the new version.' })]);
    steps.push([3800 * k, () => setUpdates(state, { state: 'restarting', detail: null })]);
  }
  const down = rollback ? 300 * k : 4000 * k;
  steps.push([down, () => {
    state.restartingUntil = Date.now() + 3200 * k;
    dropAll();
  }]);
  steps.push([down + 3200 * k, () => {
    const channel = from.channel === 'nightly' && !target.version.includes('nightly') ? 'stable' : from.channel;
    state.updates = {
      ...state.updates,
      current: { version: target.version, commit: target.commit, channel },
      previous: rollback ? null : { version: from.version },
      state: 'idle',
      detail: null,
      last_checked_at: new Date().toISOString(),
    };
    state.updates.available = availableFor(state.updates);
    record(state, { target: 'server', target_name: 'Familiar', from_version: from.version, to_version: target.version, outcome, detail: rollback ? 'Rolled back from the portal.' : null });
  }]);
  schedule('server-update', steps);
}

on('GET', '/api/admin/updates', ({ state }) => admin(state) ?? json(state.updates));
on('PUT', '/api/admin/updates', ({ state, body }) => {
  const gate = admin(state) ?? sensitive(state);
  if (gate) return gate;
  if (!state.updates.enabled) return fail(409, 'conflict', state.updates.disabled_reason ?? 'Updates are off for this build.');
  const settings = { ...state.updates.settings };
  const channels = ['stable', 'nightly', 'off'];
  if ('channel' in body) {
    if (!channels.includes(String(body.channel))) return fail(400, 'invalid', 'Choose Stable, Nightly or Off.', {}, 'channel');
    settings.channel = body.channel as typeof settings.channel;
  }
  if ('guest_default_channel' in body) {
    if (!channels.includes(String(body.guest_default_channel))) return fail(400, 'invalid', 'Choose Stable, Nightly or Off.', {}, 'guest_default_channel');
    settings.guest_default_channel = body.guest_default_channel as typeof settings.guest_default_channel;
  }
  if ('auto_install' in body) settings.auto_install = !!body.auto_install;
  if ('guest_auto_install' in body) settings.guest_auto_install = !!body.guest_auto_install;
  if ('window_start_hour' in body) {
    const hour = Number(body.window_start_hour);
    if (!Number.isInteger(hour) || hour < 0 || hour > 23) return fail(400, 'invalid', 'Choose an hour from 0 to 23.', {}, 'window_start_hour');
    settings.window_start_hour = hour;
  }
  state.updates = { ...state.updates, settings };
  state.updates.available = availableFor(state.updates);
  for (const instance of state.instances) if (instance.agent) update(state, instance, { agent: resolveAgent(state, instance.agent) });
  emit('@admin', 'updates', state.updates);
  return json(state.updates);
});
on('POST', '/api/admin/updates/check', async ({ state }) => {
  const gate = admin(state);
  if (gate) return gate;
  if (!state.updates.enabled) return fail(409, 'conflict', state.updates.disabled_reason ?? 'Updates are off for this build.');
  setUpdates(state, { state: 'checking' });
  await new Promise((resolve) => setTimeout(resolve, typeof navigator !== 'undefined' && navigator.webdriver ? 200 : 900));
  setUpdates(state, { state: 'idle', available: availableFor(state.updates), last_checked_at: new Date().toISOString() });
  return json(state.updates);
});
on('POST', '/api/admin/updates/install', ({ state }) => {
  const gate = admin(state) ?? sensitive(state);
  if (gate) return gate;
  const available = state.updates.available;
  if (!state.updates.enabled || !available || available.version === state.updates.current.version) return fail(409, 'conflict', 'There’s no update to install. Check for updates first.');
  if (state.updates.state !== 'idle' && state.updates.state !== 'failed') return fail(409, 'conflict', 'An update is already in progress.');
  runInstall(state, available, 'installed');
  return json(state.updates);
});
on('POST', '/api/admin/updates/rollback', ({ state }) => {
  const gate = admin(state) ?? sensitive(state);
  if (gate) return gate;
  const previous = state.updates.previous;
  if (!previous) return fail(409, 'conflict', 'There’s no earlier version to go back to.');
  const release = [...data.STABLE, ...data.nightlies()].find((item) => item.version === previous.version) ?? { version: previous.version, commit: '0000000', published_at: new Date().toISOString(), notes: null, url: null };
  runInstall(state, release, 'rolled_back');
  return json(state.updates);
});

registerExtras(on, { json, fail, me, mine, admin, sensitive, update });
registerEncryption(on, { json, fail, mine, sensitive, update });

const OPEN = new Set(['/api/session', '/api/setup', '/api/login', '/api/login/totp']);

export async function handle(state: DemoState, method: string, url: URL, body: Record<string, unknown>, headers: Headers): Promise<Result> {
  const path = url.pathname;
  const isPublic = OPEN.has(path) || path.startsWith('/api/invites/');
  if (!isPublic && !state.signedIn) return fail(401, 'unauthenticated', 'Sign in to continue.');
  if (!isPublic && method !== 'GET' && headers.get('X-Dot-CSRF') !== state.csrf) return fail(403, 'csrf', 'This page is out of date. Reload and try again.');
  const user = me(state);
  if (!isPublic && user && state.requireTotp && !user.totp_enabled && !path.startsWith('/api/account') && path !== '/api/logout') {
    return fail(403, 'totp_setup_required', 'Turn on two-factor authentication to keep using Familiar.');
  }
  const locked = lockedGuard(state, method, path, body, { fail, mine });
  if (locked) return locked;
  for (const [verb, pattern, handler] of routes) {
    if (verb !== method) continue;
    const match = path.match(pattern);
    if (match) return handler({ state, method, path, query: url.searchParams, body, headers }, ...match.slice(1));
  }
  return fail(404, 'not_found', `The demo server has no ${method} ${path}.`);
}

