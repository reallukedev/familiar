// SYNTHETIC demo endpoints for approval modes and the Review list (docs/home/API.md
// "Approval modes" and "Review"). Development and tests only; the changes are invented.
import type { ApprovalMode, Instance, PermissionGroup, PermissionMode, ReviewItem } from '../lib/types';
import { emit } from './events';
import type { DemoState } from './state';

type Result = { status: number; body?: unknown; blob?: Blob };
type Ctx = { state: DemoState; query: URLSearchParams; body: Record<string, unknown> };
type On = (method: string, pattern: string, handler: (ctx: Ctx, ...params: string[]) => Result | Promise<Result>) => void;
type Helpers = {
  json: (body: unknown, status?: number) => Result;
  fail: (status: number, code: string, message: string, extra?: Record<string, unknown>, field?: string) => Result;
  mine: (state: DemoState, id: string) => Instance | null;
};

const MODES = ['ask', 'changes', 'all'] as const;
type Chosen = (typeof MODES)[number];

const reviews = new WeakMap<DemoState, Record<string, ReviewItem[]>>();
const storedMode = new WeakMap<DemoState, Record<string, Chosen>>();

function itemsOf(state: DemoState, id: string): ReviewItem[] {
  let all = reviews.get(state);
  if (!all) reviews.set(state, (all = {}));
  return (all[id] ??= []);
}

const readOnly = (group: PermissionGroup) => group.tools.length > 0 && group.tools.every((tool) => tool.risk === 'read');

/** The same mapping as the server: Never stays Never; read-only groups are allowed under "changes". */
export function groupMode(mode: Chosen, group: PermissionGroup): PermissionMode {
  // App servers stay as the owner set them: a mode never turns one on.
  if (group.mode === 'deny' || group.id.startsWith('mcp:')) return group.mode;
  if (mode === 'ask') return 'ask';
  if (mode === 'all') return 'allow';
  return readOnly(group) ? 'allow' : 'ask';
}

export function detectMode(groups: PermissionGroup[], preferred?: Chosen): ApprovalMode {
  const matches = (mode: Chosen) => groups.every((group) => group.mode === groupMode(mode, group));
  const order = preferred ? [preferred, ...MODES] : [...MODES];
  return order.find(matches) ?? 'custom';
}

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

/** A worked day for the first Mac: two sessions waiting, a few already looked at. */
export function seedReview(state: DemoState, instance: Instance) {
  if (instance.id !== 'studio' || !instance.guest) return;
  const base = { instance_id: instance.id, activity_id: null, decided_at: null, delivered_at: null, note: null } as const;
  const claude = { client: 'Claude', session: 'sess-claude-1', connection_id: 'grant-claude' };
  const code = { client: 'Claude Code', session: 'sess-code-7', connection_id: 'grant-code' };
  const items: ReviewItem[] = [
    { ...base, ...claude, id: 'rv-1', tool: 'files_write', title: 'Change a file', summary: 'Wrote ~/Documents/Trip to Lisbon.md (2.1 KB)', created_at: ago(52), status: 'pending' },
    { ...base, ...claude, id: 'rv-2', tool: 'calendar_create', title: 'Create an event', summary: 'Added “Flight to Lisbon” on Friday, Oct 30 at 7:40 AM to Home', created_at: ago(49), status: 'pending' },
    { ...base, ...claude, id: 'rv-3', tool: 'safari_fill', title: 'Fill a form in Safari', summary: 'Filled the passenger form on tap.example.test (name, date of birth)', created_at: ago(44), status: 'pending' },
    { ...base, ...code, id: 'rv-4', tool: 'mac_shell', title: 'Run a command', summary: 'Ran “brew upgrade node” in Terminal', created_at: ago(26 * 60), status: 'pending' },
    { ...base, ...code, id: 'rv-5', tool: 'files_move', title: 'Move or rename', summary: 'Moved 14 screenshots from Desktop to ~/Pictures/Screenshots', created_at: ago(25 * 60), status: 'pending' },
    { ...base, ...claude, id: 'rv-6', tool: 'clipboard_set', title: 'Set the clipboard', summary: 'Copied the confirmation code', created_at: ago(3 * 60), status: 'accepted', decided_at: ago(2 * 60) },
    { ...base, ...code, id: 'rv-7', tool: 'files_delete', title: 'Move to Trash', summary: 'Moved ~/Downloads/old-invoices to the Trash', created_at: ago(28 * 60), status: 'declined', note: 'Keep invoices. Put them back and file them under Documents/Taxes instead.', decided_at: ago(27 * 60), delivered_at: ago(26.5 * 60) },
    { ...base, ...claude, id: 'rv-8', tool: 'mail_send', title: 'Send email', summary: 'Sent “Lunch Friday?” to sam@example.com', created_at: ago(30 * 60), status: 'declined', note: null, decided_at: ago(29 * 60) },
  ];
  itemsOf(state, instance.id).push(...items);
  instance.counts.pending_review = items.filter((item) => item.status === 'pending').length;
}

function recount(state: DemoState, instance: Instance) {
  instance.counts.pending_review = itemsOf(state, instance.id).filter((item) => item.status === 'pending').length;
  emit(instance.id, 'instance', instance);
}

/** Registered before the per-group permission route, so `mode` isn't taken for a group id. */
/** Apply a mode chosen when a Mac is created. */
export function chooseMode(state: DemoState, id: string, mode: unknown) {
  const chosen: Chosen = MODES.includes(mode as Chosen) ? (mode as Chosen) : 'changes';
  for (const group of state.permissions[id] ?? []) group.mode = groupMode(chosen, group);
  let stored = storedMode.get(state);
  if (!stored) storedMode.set(state, (stored = {}));
  stored[id] = chosen;
}

export function registerReview(on: On, { json, fail, mine }: Helpers) {
  const notFound = () => fail(404, 'not_found', 'That Mac doesn’t exist, or it isn’t yours.');

  on('GET', '/api/instances/:id/permissions/mode', ({ state }, id) => {
    if (!mine(state, id)) return notFound();
    return json({ mode: detectMode(state.permissions[id] ?? [], storedMode.get(state)?.[id]) });
  });
  on('PUT', '/api/instances/:id/permissions/mode', ({ state, body }, id) => {
    if (!mine(state, id)) return notFound();
    const mode = body.mode as Chosen;
    if (!MODES.includes(mode)) return fail(400, 'invalid', 'Choose Ask for Everything, Ask Before Changes or Allow Everything.', {}, 'mode');
    const groups = state.permissions[id] ?? [];
    for (const group of groups) group.mode = groupMode(mode, group);
    let stored = storedMode.get(state);
    if (!stored) storedMode.set(state, (stored = {}));
    stored[id] = mode;
    return json({ mode: detectMode(groups, mode), groups });
  });

  on('GET', '/api/instances/:id/review', ({ state, query }, id) => {
    if (!mine(state, id)) return notFound();
    const status = query.get('status') ?? 'pending';
    if (status !== 'pending' && status !== 'done') return fail(400, 'invalid', 'Use pending or done.', {}, 'status');
    const all = itemsOf(state, id);
    if (status === 'pending') return json(all.filter((item) => item.status === 'pending').toSorted((a, b) => b.created_at.localeCompare(a.created_at)));
    return json(all.filter((item) => item.status !== 'pending').toSorted((a, b) => (b.decided_at ?? '').localeCompare(a.decided_at ?? '')));
  });
  on('POST', '/api/instances/:id/review/accept-all', ({ state, body }, id) => {
    const instance = mine(state, id);
    if (!instance) return notFound();
    const ids = Array.isArray(body.ids) ? (body.ids as string[]) : null;
    const session = typeof body.session === 'string' ? body.session : null;
    const now = new Date().toISOString();
    let accepted = 0;
    for (const item of itemsOf(state, id)) {
      if (item.status !== 'pending') continue;
      if (ids ? !ids.includes(item.id) : session !== null && item.session !== session) continue;
      item.status = 'accepted';
      item.decided_at = now;
      accepted += 1;
    }
    if (accepted) {
      emit(id, 'review', { accepted });
      recount(state, instance);
    }
    return json({ accepted });
  });
  on('POST', '/api/instances/:id/review/:item', ({ state, body }, id, itemId) => {
    const instance = mine(state, id);
    const item = itemsOf(state, id).find((entry) => entry.id === itemId);
    if (!instance || !item) return notFound();
    if (body.decision !== 'accept' && body.decision !== 'decline') return fail(400, 'invalid', 'Use accept or decline.', {}, 'decision');
    if (item.status !== 'pending') return fail(409, 'conflict', `This change was already ${item.status}.`);
    const note = body.decision === 'decline' && typeof body.note === 'string' ? body.note.trim() : '';
    if (note.length > 1000) return fail(400, 'invalid', 'Keep the note under 1000 characters.', {}, 'note');
    item.status = body.decision === 'accept' ? 'accepted' : 'declined';
    item.note = note || null;
    item.decided_at = new Date().toISOString();
    emit(id, 'review', item);
    recount(state, instance);
    // The assistant picks the decline up with its next call.
    if (item.status === 'declined') {
      setTimeout(() => {
        item.delivered_at = new Date().toISOString();
        emit(id, 'review', item);
      }, 8000);
    }
    return json(item);
  });
}
