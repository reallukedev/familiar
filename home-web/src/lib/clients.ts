// The assistants the Connections page knows how to set up, and the commands and
// config files each one needs. Pure, so the snippets are unit tested.
//
// Steps were checked against each client's own docs in October 2026:
//   ChatGPT      developers.openai.com/apps-sdk/deploy/connect-chatgpt
//   Claude       support.claude.com/en/articles/11175166
//   Claude Code  code.claude.com/docs/en/mcp
//   Cursor       cursor.com/docs/context/mcp
//   VS Code      code.visualstudio.com/docs/copilot/customization/mcp-servers
//   Codex        learn.chatgpt.com/docs/extend/mcp
//   Meta Muse    Meta hasn't published setup docs for custom servers yet. Its own write-up
//                says Muse builds custom connectors when asked in a chat, and every guide
//                we found (Parallel, CData, AnythingMCP) does it that way on muse.ai with
//                an API key in the connector card. OAuth isn't documented, so this guide
//                uses a token. Check again when Meta's help pages cover it.
// Where a client moves its menus between versions, the step names both places
// rather than guessing which one a person has.

import type { AccessToken, Connections, Grant } from './types';

export type ClientId = 'chatgpt' | 'claude' | 'muse' | 'cursor' | 'vscode' | 'claude-code' | 'codex' | 'other';
export type ClientKind = 'chat' | 'editor' | 'terminal' | 'other';
/** oauth: the client signs in through Familiar. token: you paste a personal access token. */
export type ClientAuth = 'oauth' | 'token';

/** What a guide is built from: the real values for this Mac. */
export type GuideContext = {
  url: string;
  /** The Mac's display name, for connector names people see in their assistant. */
  mac: string;
  /** A config-safe id for the server entry, derived from the Mac's name. */
  server: string;
  /** The new token's secret, while the page still has it. */
  token: string | null;
};

export type Snippet = {
  /** Where it goes: "Terminal", "~/.cursor/mcp.json". */
  label: string;
  /** Spoken name for the copy button. */
  what: string;
  language: 'shell' | 'json' | 'toml' | 'text';
  code: string;
  /** Whether the code contains the token (so it can't be used until one exists). */
  usesToken: boolean;
};

/** A step's inline extra: the server address, a name to type, the token button, or a snippet. */
export type StepBlock = { type: 'url' } | { type: 'name'; value: string } | { type: 'token' } | { type: 'snippet'; snippet: Snippet };

/** Step text uses **bold** for menu items and labels and `code` for things to type. */
export type Step = { text: string; block?: StepBlock };
export type Trouble = { title: string; body: string };

export type Guide = {
  /** One line under the client's name: what it costs and how it signs in. */
  intro: string;
  /** Who can use it: plans, admin approval. Absent when nothing limits it. */
  needs?: string;
  steps: Step[];
  /** How to tell it worked, from inside the client. */
  check: string;
  trouble: Trouble[];
};

export type Client = {
  id: ClientId;
  name: string;
  /** What the token gets called when created from this guide. */
  tokenName: string;
  kind: ClientKind;
  auth: ClientAuth;
  /** Short line for the picker. */
  detail: string;
  /** Whether a grant's client_name or a token's name belongs to this client. */
  owns: (name: string) => boolean;
  guide: (context: GuideContext) => Guide;
};

export const TOKEN_PLACEHOLDER = '<token>';
const tokenOr = (token: string | null) => token ?? TOKEN_PLACEHOLDER;

/** "Studio Mac" → "studio-mac": safe as a CLI argument, a JSON key and a bare TOML key. */
export function serverId(macName: string): string {
  const slug = macName
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
    .replace(/-+$/, '');
  return slug || 'dot-home';
}

/** Single-quote a value for a POSIX shell. */
export function shellQuote(value: string): string {
  return /^[\w@%+=:,./-]+$/.test(value) ? value : `'${value.replace(/'/g, `'\\''`)}'`;
}

export function claudeCodeCommand(server: string, url: string, token: string | null): string {
  return `claude mcp add --transport http --scope user ${shellQuote(server)} ${shellQuote(url)} --header ${shellQuote(`Authorization: Bearer ${tokenOr(token)}`)}`;
}

export function cursorConfig(server: string, url: string, token: string | null): string {
  return JSON.stringify({ mcpServers: { [server]: { url, headers: { Authorization: `Bearer ${tokenOr(token)}` } } } }, null, 2);
}

export function codexCommands(server: string, url: string): string {
  return `codex mcp add ${shellQuote(server)} --url ${shellQuote(url)}\ncodex mcp login ${shellQuote(server)}`;
}

export function codexConfig(server: string, url: string): string {
  return `[mcp_servers.${server}]\nurl = ${JSON.stringify(url)}`;
}

export function bearerHeader(token: string | null): string {
  return `Authorization: Bearer ${tokenOr(token)}`;
}

/** The value of the Authorization header, for clients that ask for it on its own. */
export function bearerValue(token: string | null): string {
  return `Bearer ${tokenOr(token)}`;
}

/** What to send Muse in a chat so it builds the connector. No secret in it: the key goes in Muse's own prompt. */
export function museRequest(mac: string, url: string): string {
  return `Build a custom integration to my Mac, “${mac}”. It’s a remote MCP server over streamable HTTP at ${url}. It needs an API key, sent as the Authorization header. Test each of its tools, then save the integration as a reusable skill.`;
}

const lower = (name: string) => name.trim().toLowerCase();

// Problems that apply to every client.
const wrongAddress: Trouble = {
  title: 'It can’t reach the server',
  body: 'Check the address ends in `/mcp` and starts with `https://`. Open it in a browser on the same computer: an Unauthorized or 401 error means the address is right, and “can’t be reached” means it isn’t.',
};
const paused: Trouble = {
  title: 'Every request is refused',
  body: 'Assistants are paused on this Mac. Resume them on the Mac’s Overview page. While paused, assistants can connect but can’t do anything.',
};
const approvals: Trouble = {
  title: 'It says it’s waiting for approval',
  body: 'Some actions, like installing apps, ask you first. The request shows up in Familiar with a badge on this Mac. Approve it there, then ask the assistant to try again.',
};
const unauthorized: Trouble = {
  title: 'It says 401 or Unauthorized',
  body: 'The token was revoked, has expired, or was copied incompletely. Create a new one here and replace the old one in your setup. A token only works for the Mac it was made on.',
};
const publicOnly = (client: string): Trouble => ({
  title: `${client} can’t connect, but other apps can`,
  body: `${client} connects from its own servers on the internet, not from your computer. Familiar must be reachable at this address from outside your network. An address that only works on your Wi-Fi or over a VPN like Tailscale won’t work for ${client}.`,
});
const signInLoop: Trouble = {
  title: 'Sign-in doesn’t finish',
  body: 'Sign in to Familiar in the same browser first, then try again. If you chose a different Mac when asked, disconnect it under Connected apps and connect again.',
};

export const CLIENTS: readonly Client[] = [
  {
    id: 'chatgpt',
    name: 'ChatGPT',
    tokenName: 'ChatGPT',
    kind: 'chat',
    auth: 'oauth',
    detail: 'Signs in with Familiar',
    owns: (name) => /chatgpt|openai/.test(lower(name)) && !/codex/.test(lower(name)),
    guide: ({ mac }) => ({
      intro: 'Add this Mac to ChatGPT as a custom MCP server. ChatGPT signs in through Familiar, so there’s no token to copy.',
      needs: 'Needs ChatGPT Plus, Pro, Business, Enterprise or Edu on the web. On Business and Enterprise, a workspace admin may need to allow custom servers first.',
      steps: [
        {
          text: 'On chatgpt.com, add a custom MCP server. Depending on your version of ChatGPT, it’s at **chatgpt.com/plugins** › **+** › **Add custom MCP server**, or **Settings** › **Apps** › **Advanced settings** › **Developer mode**, then **Create app**.',
        },
        { text: 'Name it after this Mac.', block: { type: 'name', value: mac } },
        { text: 'Paste the server address.', block: { type: 'url' } },
        { text: 'For authentication, choose **OAuth**. Confirm the warning about custom servers, then create it.' },
        { text: 'ChatGPT opens Familiar. Sign in if asked, choose **' + mac + '**, and select **Allow**.' },
      ],
      check: 'In a new chat, type `@` and pick ' + mac + ', then ask “What’s on my Mac’s screen?”',
      trouble: [publicOnly('ChatGPT'), signInLoop, wrongAddress, paused, approvals],
    }),
  },
  {
    id: 'claude',
    name: 'Claude',
    tokenName: 'Claude',
    kind: 'chat',
    auth: 'oauth',
    detail: 'Claude.ai and Claude Desktop',
    owns: (name) => lower(name).startsWith('claude') && !/code/.test(lower(name)),
    guide: ({ mac }) => ({
      intro: 'Add this Mac to Claude as a custom connector. It works in Claude on the web and in Claude Desktop, and Claude signs in through Familiar.',
      needs: 'Every Claude plan can add one; the Free plan is limited to one custom connector. On Team and Enterprise, an owner adds it under **Organization settings** › **Connectors**, and then you select **Connect**.',
      steps: [
        { text: 'In Claude, open **Customize** › **Connectors** (in older versions, **Settings** › **Connectors**), then select **+ Add** › **Add custom connector**.' },
        { text: 'Name it after this Mac.', block: { type: 'name', value: mac } },
        { text: 'Paste the server address.', block: { type: 'url' } },
        { text: 'If Claude asks how to register its OAuth client, choose **Register automatically**. Then select **Add**.' },
        { text: 'Claude opens Familiar. Sign in if asked, choose **' + mac + '**, and select **Allow**.' },
      ],
      check: 'In a chat, select **+** › **Connectors** and turn on ' + mac + ', then ask “What’s on my Mac’s screen?”',
      trouble: [
        publicOnly('Claude'),
        signInLoop,
        { title: 'You need to change how it signs in', body: 'Claude doesn’t let you edit a connector’s sign-in settings. Remove the connector and add it again.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'muse',
    name: 'Meta Muse',
    tokenName: 'Meta Muse',
    kind: 'chat',
    auth: 'token',
    detail: 'Set up in a chat on muse.ai',
    owns: (name) => /\bmuse\b/.test(lower(name)),
    guide: ({ mac, url, token }) => ({
      intro: 'Muse adds custom MCP servers when you ask it to in a chat. You’ll make a token for Muse and give it to Muse’s secure key prompt, never the chat itself.',
      needs: 'Needs a Muse account, which isn’t offered in every country yet. Set it up on the web at **muse.ai**; connectors added there also work in the Muse app.',
      steps: [
        { text: 'Create a token for Muse.', block: { type: 'token' } },
        {
          text: 'At **muse.ai**, start a new chat and send this.',
          block: { type: 'snippet', snippet: { label: 'Message to Muse', what: 'message', language: 'text', code: museRequest(mac, url), usesToken: false } },
        },
        {
          text: 'When Muse shows a card for the connector, select **Connect**, paste this as the API key, and select **Add**. Muse keeps it in its credential store, out of the chat.',
          block: { type: 'snippet', snippet: { label: 'API key', what: 'API key', language: 'text', code: bearerValue(token), usesToken: true } },
        },
        { text: 'Muse tests the tools it finds. When it offers to save the integration as a skill, say yes, so ' + mac + ' is there in every new chat.' },
      ],
      check: 'In a new chat, ask Muse “What’s on my Mac’s screen?”',
      trouble: [
        publicOnly('Muse'),
        { title: 'Muse asks for the key in the chat', body: 'Don’t paste it there. Ask Muse to open its secure prompt for the key instead. If a token ended up in a chat, revoke it here and make a new one.' },
        { title: 'It forgets the Mac in a new chat', body: 'The integration wasn’t saved as a skill. Ask Muse to save it as a reusable skill, then start a new chat.' },
        unauthorized,
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'cursor',
    name: 'Cursor',
    tokenName: 'Cursor',
    kind: 'editor',
    auth: 'token',
    detail: 'Personal access token',
    owns: (name) => /cursor/.test(lower(name)),
    guide: ({ server, url, token }) => ({
      intro: 'Cursor reads its MCP servers from a config file. You’ll make a token for Cursor and paste a ready-made entry into that file.',
      steps: [
        { text: 'Create a token for Cursor.', block: { type: 'token' } },
        {
          text: 'Open `~/.cursor/mcp.json` (create it if it’s missing) and paste this. If the file already lists servers, add the `' + server + '` entry inside its `mcpServers`.',
          block: { type: 'snippet', snippet: { label: '~/.cursor/mcp.json', what: 'Cursor config', language: 'json', code: cursorConfig(server, url, token), usesToken: true } },
        },
        { text: 'Save the file. Cursor picks up the change; if it doesn’t, restart Cursor.' },
      ],
      check: 'In Cursor’s MCP list, ' + server + ' should be on and show its tools. Then ask the agent “What’s on my Mac’s screen?”',
      trouble: [
        unauthorized,
        { title: 'The server shows an error in Cursor', body: 'Open the Output panel (⇧⌘U) and choose **MCP Logs** to see why. A JSON mistake in `mcp.json`, like a missing comma, stops every server in the file.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'vscode',
    name: 'VS Code',
    tokenName: 'VS Code',
    kind: 'editor',
    auth: 'oauth',
    detail: 'GitHub Copilot agent mode',
    owns: (name) => /vs ?code|visual studio code/.test(lower(name)),
    guide: ({ server, mac }) => ({
      intro: 'Add this Mac as an MCP server for GitHub Copilot’s agent mode. VS Code signs in through Familiar, so there’s no token to copy.',
      steps: [
        { text: 'Open the Command Palette (⇧⌘P, or Ctrl+Shift+P) and run **MCP: Add Server**.' },
        { text: 'Choose **HTTP**, then paste the server address.', block: { type: 'url' } },
        { text: 'For the server ID, type this. Then choose to save it globally so every workspace has it.', block: { type: 'name', value: server } },
        { text: 'When VS Code asks to sign in, allow it. Familiar opens: sign in if asked, choose **' + mac + '**, and select **Allow**.' },
      ],
      check: 'Run **MCP: List Servers** and check ' + server + ' is running. In Copilot Chat, switch to Agent and select **Configure Tools** to see its tools.',
      trouble: [
        signInLoop,
        { title: 'The server won’t start', body: 'Run **MCP: List Servers**, select ' + server + ', and choose **Show Output** to see the error.' },
        { title: 'Your organization blocks it', body: 'Copilot Business and Enterprise can limit which MCP servers are allowed. Ask whoever manages Copilot for your organization.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'claude-code',
    name: 'Claude Code',
    tokenName: 'Claude Code',
    kind: 'terminal',
    auth: 'token',
    detail: 'Personal access token',
    owns: (name) => /claude code/.test(lower(name)),
    guide: ({ server, url, token }) => ({
      intro: 'Add this Mac to Claude Code with one command. It uses a token, so it also works over SSH and on computers without a browser.',
      steps: [
        { text: 'Create a token for Claude Code.', block: { type: 'token' } },
        {
          text: 'Run this in Terminal. It adds the Mac for every project on this computer.',
          block: { type: 'snippet', snippet: { label: 'Terminal', what: 'command', language: 'shell', code: claudeCodeCommand(server, url, token), usesToken: true } },
        },
      ],
      check: 'Run `claude mcp list`: ' + server + ' should say Connected. Inside Claude Code, `/mcp` shows its tools.',
      trouble: [
        unauthorized,
        { title: 'It was already added', body: 'Remove the old entry with `claude mcp remove ' + server + ' --scope user`, then run the command again.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'codex',
    name: 'Codex',
    tokenName: 'Codex',
    kind: 'terminal',
    auth: 'oauth',
    detail: 'CLI, IDE extension and app',
    owns: (name) => /codex/.test(lower(name)),
    guide: ({ server, url, mac }) => ({
      intro: 'Add this Mac to OpenAI’s Codex. The CLI, the IDE extension and the app share one config, so adding it once covers all three.',
      steps: [
        {
          text: 'Run these in Terminal. The first adds the server; the second opens your browser to sign in.',
          block: { type: 'snippet', snippet: { label: 'Terminal', what: 'commands', language: 'shell', code: codexCommands(server, url), usesToken: false } },
        },
        { text: 'Familiar opens: sign in if asked, choose **' + mac + '**, and select **Allow**.' },
        {
          text: 'Prefer editing the config yourself? Add this to `~/.codex/config.toml` instead, then run `codex mcp login ' + server + '`.',
          block: { type: 'snippet', snippet: { label: '~/.codex/config.toml', what: 'Codex config', language: 'toml', code: codexConfig(server, url), usesToken: false } },
        },
      ],
      check: 'Run `codex mcp list`, or type `/mcp` inside Codex: ' + server + ' should be listed with its tools.',
      trouble: [
        signInLoop,
        { title: 'Sign-in opens on the wrong computer', body: 'Sign-in needs a browser on the computer running Codex. On a server over SSH, use the Any MCP client guide with a token instead.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
  {
    id: 'other',
    name: 'Any MCP client',
    tokenName: 'MCP client',
    kind: 'other',
    auth: 'token',
    detail: 'OAuth or a token',
    owns: () => false,
    guide: ({ token }) => ({
      intro: 'Familiar speaks MCP over streamable HTTP. Any client that supports remote servers can connect, either by signing in or with a token.',
      steps: [
        { text: 'Give your client the server address.', block: { type: 'url' } },
        { text: 'If the client supports OAuth, let it sign in. It finds Familiar’s sign-in on its own (OAuth 2.1 with PKCE and automatic client registration), then you choose this Mac and allow it.' },
        { text: 'If it doesn’t, create a token and send it as a header with every request.', block: { type: 'token' } },
        {
          text: 'The header to send:',
          block: { type: 'snippet', snippet: { label: 'HTTP header', what: 'header', language: 'text', code: bearerHeader(token), usesToken: true } },
        },
      ],
      check: 'Ask your client to list its tools. You should see tools like `mac_screenshot` and `mac_click`.',
      trouble: [
        unauthorized,
        { title: 'It asks for a client ID', body: 'Familiar registers clients automatically. If your client can’t, use a token instead.' },
        wrongAddress,
        paused,
        approvals,
      ],
    }),
  },
];

export const CLIENT_GROUPS: { kind: ClientKind; title: string }[] = [
  { kind: 'chat', title: 'Chat apps' },
  { kind: 'editor', title: 'Code editors' },
  { kind: 'terminal', title: 'Terminal' },
  { kind: 'other', title: 'Other' },
];

export function findClient(id: string | null | undefined): Client | undefined {
  return CLIENTS.find((client) => client.id === id);
}

/** "Claude Code", or "Claude Code 2" when that name is taken. */
export function uniqueTokenName(base: string, tokens: readonly AccessToken[]): string {
  const taken = new Set(tokens.map((token) => lower(token.name)));
  if (!taken.has(lower(base))) return base;
  let n = 2;
  while (taken.has(lower(`${base} ${n}`))) n += 1;
  return `${base} ${n}`;
}

/**
 * Whether a client has reached this Mac, from data the server already reports:
 * grants appear when someone allows an OAuth client, and a token's or grant's
 * `last_used_at` is set on its first request. Nothing here is guessed.
 */
export type ClientStatus =
  | { state: 'waiting' }
  /** Allowed in Familiar, but no request yet. */
  | { state: 'allowed'; grant: Grant }
  | { state: 'connected'; via: 'grant'; grant: Grant; fresh: boolean }
  | { state: 'connected'; via: 'token'; token: AccessToken; fresh: boolean }
  /** The token made in this guide is gone (revoked here or elsewhere). */
  | { state: 'revoked' };

export function clientStatus(client: Client, data: Connections | undefined, watch: { grantIds: ReadonlySet<string> | null; tokenId: string | null }): ClientStatus {
  if (!data) return { state: 'waiting' };
  if (watch.tokenId) {
    const token = data.tokens.find((item) => item.id === watch.tokenId);
    if (!token) return { state: 'revoked' };
    if (token.last_used_at) return { state: 'connected', via: 'token', token, fresh: true };
    return { state: 'waiting' };
  }
  if (client.auth === 'oauth' && watch.grantIds) {
    // Any grant that appeared while this guide was open is this person finishing the steps.
    const added = data.grants.find((grant) => !watch.grantIds!.has(grant.id));
    if (added) return added.last_used_at ? { state: 'connected', via: 'grant', grant: added, fresh: true } : { state: 'allowed', grant: added };
  }
  const grant = data.grants.find((item) => client.owns(item.client_name) && item.last_used_at);
  if (grant) return { state: 'connected', via: 'grant', grant, fresh: false };
  const token = data.tokens.find((item) => client.owns(item.name) && item.last_used_at);
  if (token) return { state: 'connected', via: 'token', token, fresh: false };
  return { state: 'waiting' };
}
