// SYNTHETIC demo endpoint for MCP servers that apps on the Mac provide (docs/home/API.md
// "App servers"). Development and tests only; the servers and tools are invented.
import type { AppServer, Instance, PermissionGroup } from '../lib/types';
import type { DemoState } from './state';

type Result = { status: number; body?: unknown };
type Ctx = { state: DemoState; body: Record<string, unknown> };
type On = (method: string, pattern: string, handler: (ctx: Ctx, ...params: string[]) => Result | Promise<Result>) => void;
type Helpers = {
  json: (body: unknown, status?: number) => Result;
  fail: (status: number, code: string, message: string) => Result;
  mine: (state: DemoState, id: string) => Instance | null;
};

type Found = { id: string; label: string; description: string; server: Omit<AppServer, 'tools_error' | 'listed_at' | 'changed'>; tools: PermissionGroup['tools']; error?: string };

// What the first Mac has: two apps with their own servers, and one listed in Claude's settings.
const STUDIO: Found[] = [
  {
    id: 'xcode',
    label: 'Xcode',
    description: 'Xcode’s own MCP server.',
    server: { app: 'Xcode', bundle_id: 'com.apple.dt.Xcode', transport: 'stdio', detail: 'xcrun mcpbridge', source: 'app' },
    tools: [
      { name: 'mcp_xcode_BuildProject', title: 'Build Project', risk: 'write' },
      { name: 'mcp_xcode_RunTests', title: 'Run Tests', risk: 'write' },
      { name: 'mcp_xcode_GetBuildLog', title: 'Get Build Log', risk: 'read' },
      { name: 'mcp_xcode_XcodeListWindows', title: 'List Windows', risk: 'read' },
    ],
  },
  {
    id: 'figma',
    label: 'Figma',
    description: 'Figma’s own MCP server.',
    server: { app: 'Figma', bundle_id: 'com.figma.Desktop', transport: 'http', detail: 'http://127.0.0.1:3845/mcp', source: 'app' },
    tools: [],
    error: 'Figma isn’t answering. Open Figma and turn on its MCP server, then look again.',
  },
  {
    id: 'things',
    label: 'things',
    description: 'Listed in Claude’s MCP settings.',
    server: { app: null, bundle_id: null, transport: 'stdio', detail: 'npx -y things-mcp', source: 'Claude' },
    tools: [
      { name: 'mcp_things_get_today', title: 'Get Today', risk: 'read' },
      { name: 'mcp_things_add_todo', title: 'Add To-Do', risk: 'write' },
    ],
  },
];

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function group(found: Found, mode: PermissionGroup['mode'], listed: boolean): PermissionGroup {
  const on = mode !== 'deny';
  return {
    id: `mcp:${found.id}`,
    label: found.label,
    description: found.description,
    mode,
    default_mode: 'deny',
    tools: on && listed && !found.error ? found.tools : [],
    server: { ...found.server, changed: false, tools_error: on && found.error ? found.error : null, listed_at: on && listed && !found.error ? new Date().toISOString() : null },
  };
}

/** Lists a server's tools when it's turned on, as the real server does. */
export function onAppServerMode(target: PermissionGroup) {
  if (!target.id.startsWith('mcp:')) return;
  const found = STUDIO.find((item) => `mcp:${item.id}` === target.id);
  if (found) Object.assign(target, group(found, target.mode, true));
}

export function registerAppServers(on: On, { json, fail, mine }: Helpers) {
  on('POST', '/api/instances/:id/app-servers/refresh', async ({ state }, id) => {
    const instance = mine(state, id);
    if (!instance) return fail(404, 'not_found', 'That Mac doesn’t exist, or it isn’t yours.');
    await wait(700);
    if (instance.state !== 'ready' && instance.state !== 'degraded') return fail(503, 'unavailable', 'The Mac isn’t running. Start it to look for app servers.');
    const groups = (state.permissions[id] ??= []);
    const found = instance.id === 'studio' ? STUDIO : [];
    for (const item of found) {
      const existing = groups.find((g) => g.id === `mcp:${item.id}`);
      if (existing) Object.assign(existing, group(item, existing.mode, true));
      else groups.push(group(item, 'deny', false));
    }
    return json(groups);
  });
}
