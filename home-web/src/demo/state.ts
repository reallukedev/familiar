// SYNTHETIC DEMO STATE (development and tests only).
import type { Activity, Approval, AppEntry, Connections, ImageStatus, Instance, Invite, OAuthRequest, PermissionGroup, SecurityEvent, Snapshot, UpdatesStatus } from '../lib/types';
import * as data from './data';
import { createExtras, type ExtrasState } from './extras';
import { seedReview } from './review';

export type Scenario = 'one' | 'empty' | 'many' | 'errors' | 'booting' | 'admin' | 'member' | 'firstrun' | 'signedout' | 'offline' | 'longnames' | 'noguest' | 'approvals' | 'enroll'
  | 'updates' | 'updates-available' | 'updates-restarting' | 'updates-disabled' | 'updates-nightly'
  | 'storage-full' | 'storage-unlimited'
  // Encryption: Familiar restarted, so encrypted Macs are locked; and an account that hasn't saved its recovery key.
  | 'locked' | 'recovery';
export const SCENARIOS: Scenario[] = ['one', 'empty', 'many', 'errors', 'booting', 'admin', 'member', 'firstrun', 'signedout', 'offline', 'longnames', 'noguest', 'approvals', 'enroll',
  'updates', 'updates-available', 'updates-restarting', 'updates-disabled', 'updates-nightly', 'storage-full', 'storage-unlimited', 'locked', 'recovery'];

export type DemoState = {
  scenario: Scenario;
  users: data.DemoUser[];
  owner: Record<string, string>;
  signedIn: string | null;
  csrf: string;
  setupRequired: boolean;
  requireTotp: boolean;
  confirmedUntil: number;
  /** A sign-in waiting for its code; a reset may hold a new recovery key for it. */
  challenge: { id: string; user: string; recoveryKey?: string } | null;
  pendingSecret: string | null;
  instances: Instance[];
  /** The image builder's Mac: reachable by id, never listed. */
  hidden: Set<string>;
  approvals: Record<string, Approval[]>;
  activity: Record<string, Activity[]>;
  apps: Record<string, AppEntry[]>;
  permissions: Record<string, PermissionGroup[]>;
  snapshots: Record<string, Snapshot[]>;
  connections: Record<string, Connections>;
  security: SecurityEvent[];
  sessions: ReturnType<typeof data.sessions>;
  invites: Invite[];
  image: ImageStatus;
  updates: UpdatesStatus;
  /** While the server "restarts" for an update, every request fails until this time. */
  restartingUntil: number;
  overviewFailing: boolean;
  oauth: Record<string, OAuthRequest>;
  offline: boolean;
  latency: number;
  /** The signed-in person's data key isn't unlocked (Familiar "restarted"); see encryption.ts. */
  locked: boolean;
  /** They haven't confirmed saving a recovery key. */
  recoveryKeyNeeded: boolean;
  /** Storage, preferences, files, clipboard (see extras.ts). */
  extras: ExtrasState;
};

function prepare(state: DemoState, instance: Instance, owner: string, seed: number, activityCount = 80) {
  state.instances.push(instance);
  state.owner[instance.id] = owner;
  state.activity[instance.id] = instance.guest && instance.counts.calls_24h ? data.activity(seed, activityCount) : [];
  state.approvals[instance.id] = data.pastApprovals(instance.id);
  state.apps[instance.id] = instance.guest ? data.apps(state.scenario === 'longnames') : [];
  state.permissions[instance.id] = data.permissions();
  state.snapshots[instance.id] = data.snapshots(instance.id);
  state.connections[instance.id] = { mcp_url: instance.mcp_url, grants: data.grants(), tokens: data.tokens() };
}

function addPending(state: DemoState, id: string, n: number) {
  const pending = Array.from({ length: n }, (_, i) => data.approval(id, i, 0.2 + i * 0.4));
  state.approvals[id] = [...pending, ...(state.approvals[id] ?? [])];
  const instance = state.instances.find((item) => item.id === id);
  if (instance) instance.counts.pending_approvals = n;
}

export function createState(scenario: Scenario, ua: string): DemoState {
  const state: DemoState = {
    scenario,
    users: data.users(),
    owner: {},
    signedIn: 'u-alex',
    csrf: 'demo-csrf-token',
    setupRequired: false,
    requireTotp: false,
    confirmedUntil: 0,
    challenge: null,
    pendingSecret: null,
    instances: [],
    hidden: new Set(),
    approvals: {},
    activity: {},
    apps: {},
    permissions: {},
    snapshots: {},
    connections: {},
    security: data.security(),
    sessions: data.sessions(ua),
    invites: data.invites(),
    image: data.image(null),
    updates: data.updates('current'),
    restartingUntil: 0,
    overviewFailing: false,
    oauth: {
      'req-claude': { id: 'req-claude', client_name: 'Claude', redirect_host: 'claude.ai', expires_at: new Date(Date.now() + 10 * 60_000).toISOString() },
      'req-chatgpt': { id: 'req-chatgpt', client_name: 'ChatGPT', redirect_host: 'chatgpt.com', expires_at: new Date(Date.now() + 10 * 60_000).toISOString() },
    },
    offline: false,
    latency: 0,
    extras: createExtras(scenario),
    locked: false,
    recoveryKeyNeeded: false,
  };

  switch (scenario) {
    case 'firstrun':
      state.setupRequired = true;
      state.requireTotp = true;
      state.signedIn = null;
      state.users = [];
      break;
    case 'signedout':
      state.signedIn = null;
      prepare(state, data.instance('studio', 'Studio Mac', 'ready'), 'u-alex', 1);
      break;
    case 'offline':
      state.offline = true;
      break;
    case 'empty':
      state.signedIn = 'u-sam';
      break;
    case 'enroll':
      state.signedIn = 'u-sam';
      state.requireTotp = true;
      prepare(state, data.instance('sam-mac', 'Sam’s Mac', 'ready'), 'u-sam', 7);
      break;
    case 'member':
      state.signedIn = 'u-sam';
      prepare(state, data.instance('sam-mac', 'Sam’s Mac', 'ready', { counts: { calls_24h: 38, errors_24h: 0, pending_approvals: 0 } }), 'u-sam', 7);
      break;
    case 'noguest':
      // A server from before updates: no `agent` on the Mac at all.
      prepare(state, data.instance('fresh', 'New Mac', 'stopped', { guest: null, agent: undefined, counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 }, last_active_at: null }), 'u-alex', 2);
      break;
    case 'updates':
    case 'updates-available':
    case 'updates-restarting':
    case 'updates-disabled':
    case 'updates-nightly': {
      const kind = scenario === 'updates' ? 'current' : scenario === 'updates-disabled' ? 'disabled' : scenario === 'updates-nightly' ? 'nightly' : 'available';
      state.updates = data.updates(kind);
      const nightly = kind === 'nightly';
      const target = nightly ? state.updates.available!.version : data.STABLE[2]!.version;
      const running = nightly ? state.updates.current.version : kind === 'available' ? data.STABLE[1]!.version : data.STABLE[2]!.version;
      const effective = nightly ? 'nightly' : 'stable';
      prepare(state, data.instance('studio', 'Studio Mac', 'ready', { agent: data.agent({ version: running, effective_channel: effective, available: kind === 'current' || kind === 'disabled' ? null : target }) }), 'u-alex', 1);
      prepare(state, data.instance('work', 'Work Mac', 'ready', { agent: data.agent({ version: '2.6.1', effective_channel: effective, available: kind === 'disabled' ? null : target, can_update: false }) }), 'u-alex', 21, 20);
      prepare(state, data.instance('photos', 'Photo Library', 'sleeping', { agent: data.agent({ version: running, channel: 'off', effective_channel: 'off' }) }), 'u-alex', 22, 20);
      break;
    }
    case 'locked':
      // After a restart: off and hibernated encrypted Macs are locked; a running one keeps running.
      state.locked = true;
      prepare(state, data.instance('studio', 'Studio Mac', 'stopped', { locked: true, state_detail: null }), 'u-alex', 1);
      prepare(state, data.instance('photos', 'Photo Library', 'hibernated', { locked: true }), 'u-alex', 22, 20);
      prepare(state, data.instance('work', 'Work Mac', 'ready', { locked: true }), 'u-alex', 21, 20);
      prepare(state, data.instance('old', 'Old Sonoma Mac', 'stopped', { encrypted: false, can_encrypt: true }), 'u-alex', 25, 20);
      break;
    case 'recovery':
      state.recoveryKeyNeeded = true;
      prepare(state, data.instance('studio', 'Studio Mac', 'ready'), 'u-alex', 1);
      prepare(state, data.instance('old', 'Old Sonoma Mac', 'stopped', { encrypted: false, can_encrypt: true }), 'u-alex', 25, 20);
      break;
    case 'booting':
      prepare(state, data.instance('studio', 'Studio Mac', 'starting', { state_detail: 'Turning on the virtual machine.', last_active_at: null }), 'u-alex', 1);
      break;
    case 'errors': {
      prepare(state, data.instance('studio', 'Studio Mac', 'degraded', {
        state_detail: 'The Familiar agent inside the Mac stopped answering 2 minutes ago. Familiar will restart it in 3 minutes if it doesn’t recover.',
        guest: { ...data.instance('x', 'x', 'ready').guest!, hostname: 'studio.local', permissions: { accessibility: false, screen_recording: true, full_disk_access: false } },
        counts: { calls_24h: 61, errors_24h: 17, pending_approvals: 0 },
      }), 'u-alex', 3, 60);
      state.activity.studio = data.activity(3, 60, 0.35);
      prepare(state, data.instance('render', 'Render Box', 'error', { state_detail: 'QEMU exited three times in a row (code 1). It stays off until you start it. The log mentions “not enough memory to start”.' }), 'u-alex', 4);
      state.overviewFailing = true;
      state.image = data.image('failed');
      break;
    }
    case 'storage-full':
    case 'storage-unlimited':
    case 'many':
    case 'admin':
    case 'longnames':
    case 'approvals':
    case 'one':
    default: {
      prepare(state, data.instance('studio', 'Studio Mac', 'ready'), 'u-alex', 1);
      if (scenario === 'one') addPending(state, 'studio', 1);
      if (scenario === 'approvals') addPending(state, 'studio', 3);
      if (scenario === 'longnames') {
        prepare(state, data.instance('long', 'The Very Long Named Mac That Belongs To The Design Team In The Back Office', 'sleeping'), 'u-alex', 9);
      }
      if (scenario === 'many' || scenario === 'admin' || scenario === 'storage-full' || scenario === 'storage-unlimited') {
        addPending(state, 'studio', 2);
        const extra: [string, string, Instance['state'], Partial<Instance>?][] = [
          ['work', 'Work Mac', 'sleeping'],
          ['photos', 'Photo Library', 'hibernated'],
          ['build', 'Build Machine', 'booting', { state_detail: 'Waiting for macOS to finish starting.' }],
          ['test', 'Testing', 'starting'],
          ['old', 'Old Sonoma Mac', 'stopped', { encrypted: false, can_encrypt: true }],
          ['scratch', 'Scratch', 'stopping'],
          ['render', 'Render Box', 'error', { state_detail: 'QEMU exited three times in a row (code 1).' }],
          ['mini', 'Mac mini in the Office', 'degraded', { driver: 'remote', remote_host: 'mac-mini.tailnet.example.test', paused: true }],
          ['writer', 'Writing Desk', 'ready', { paused: true }],
        ];
        extra.forEach(([id, name, st, more], i) => prepare(state, data.instance(id, name, st, more), 'u-alex', 20 + i, 20));
        // Agents in every state the Settings page shows.
        const studio = state.instances.find((item) => item.id === 'studio')!;
        studio.agent = data.agent({ version: data.STABLE[1]!.version, available: data.STABLE[2]!.version });
        const writer = state.instances.find((item) => item.id === 'writer')!;
        writer.agent = data.agent({ version: '2.6.1', available: data.STABLE[2]!.version, can_update: false });
      }
      if (scenario === 'admin') {
        state.image = data.image('installing');
        prepare(state, data.instance('builder', 'macOS Tahoe Installer', 'ready', { counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 } }), 'u-alex', 40, 0);
        state.hidden.add('builder');
      }
    }
  }

  for (const instance of state.instances) seedReview(state, instance);
  for (const user of state.users) user.instance_count = Object.values(state.owner).filter((owner) => owner === user.id).length;
  return state;
}
