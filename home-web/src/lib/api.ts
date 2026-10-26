import { transport } from './transport';
import type {
  AccessToken, AccountInfo, ActivityPage, AdminInstance, AdminUser, AppEntry, Approval, Connections, HostOverview,
  ImageStatus, InstallSource, Instance, Invite, InviteInfo, OAuthRequest, PermissionGroup, PermissionMode, PowerAction,
  AgentChannel, Role, SecurityEvent, SessionInfo, Snapshot, UpdatesStatus, User,
  AdminStorage, FileItem, FolderListing, MyStorage, Preferences, StorageDefaults, StorageOverrides, SnapshotCap, SnapshotUsage,
  PushKey, PushSubscriptionBody, ApprovalMode, ReviewItem, WithRecoveryKey,
} from './types';

/** Codes the portal adds on top of the server's own. */
export type ClientErrorCode = 'network' | 'timeout' | 'reauth_cancelled' | 'bad_response';

export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  readonly field?: string;
  readonly retryAfterS?: number;
  /** Extra top-level fields from the error body (the TOTP `challenge`, for one). */
  readonly extra: Record<string, unknown>;

  constructor(code: string, message: string, status = 0, options: { field?: string; retryAfterS?: number; extra?: Record<string, unknown> } = {}) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
    this.field = options.field;
    this.retryAfterS = options.retryAfterS;
    this.extra = options.extra ?? {};
  }

  /** True for errors the person already saw or chose (no toast needed). */
  get silent(): boolean {
    return this.code === 'reauth_cancelled' || this.code === 'unauthenticated' || this.code === 'totp_setup_required';
  }
}

/** Hooks the app installs once at startup; the API layer stays free of UI. */
export type ApiHooks = {
  csrf: () => string | null;
  /** Ask the person to confirm their password. Resolves true once confirmed. */
  reauth: () => Promise<boolean>;
  /** Fetch a fresh session (and CSRF token) after a `csrf` rejection. */
  refreshSession: () => Promise<void>;
  unauthenticated: () => void;
  /** The account must enroll in two-factor before anything else works. */
  totpSetupRequired: () => void;
  reachable: (ok: boolean) => void;
  /** A reply carried a new recovery key (sign-in, setup, a password change): show it once. */
  recoveryKey: (key: string) => void;
};

export const hooks: ApiHooks = {
  csrf: () => null,
  reauth: async () => false,
  refreshSession: async () => {},
  unauthenticated: () => {},
  totpSetupRequired: () => {},
  reachable: () => {},
  recoveryKey: () => {},
};

/** Hand a reply's one-time `recovery_key` to the portal, wherever it came from. */
function noticeRecoveryKey(body: unknown) {
  if (body && typeof body === 'object' && typeof (body as { recovery_key?: unknown }).recovery_key === 'string') {
    hooks.recoveryKey((body as { recovery_key: string }).recovery_key);
  }
}

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
type Options = {
  /** Sign-in and setup calls: a 401 is an answer, not an expired session. */
  publicCall?: boolean;
  signal?: AbortSignal;
};

const OFFLINE_MESSAGE = 'Can’t reach Familiar. Check your connection; the portal will retry when it’s back.';
const TIMEOUT_MESSAGE = 'Familiar is taking too long to answer. Try again in a moment.';

/** How long a read may take before the portal gives up and shows an error. */
export const READ_TIMEOUT_MS = 20_000;
/** Changes can run longer (an app install answers within about 45 s). */
export const WRITE_TIMEOUT_MS = 60_000;
/** An upload or download fails after this long without any bytes moving. */
export const TRANSFER_STALL_MS = 60_000;

/** Below the browsers' 64 KB budget for requests that outlive the page. */
const KEEPALIVE_MAX = 60_000;

/** Every request in flight, so signing out can stop them all. */
const live = new Set<AbortController>();

/** Abort every request and upload in flight (signing out, switching accounts). */
export function abortRequests() {
  for (const controller of live) controller.abort();
  live.clear();
}

/**
 * One request's abort signal: fires when the caller's own signal does, when
 * `abortRequests` runs, or when `ms` pass without `touch()`. `expired()` tells
 * a timeout apart from a cancel.
 */
function deadline(ms: number, outer?: AbortSignal) {
  const controller = new AbortController();
  let expired = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      expired = true;
      controller.abort();
    }, ms);
  };
  const forward = () => controller.abort();
  if (outer?.aborted) controller.abort();
  else outer?.addEventListener('abort', forward, { once: true });
  arm();
  live.add(controller);
  return {
    signal: controller.signal,
    expired: () => expired,
    touch: arm,
    done() {
      clearTimeout(timer);
      outer?.removeEventListener('abort', forward);
      live.delete(controller);
    },
  };
}

/** Settle with `work`, or reject with an AbortError once `signal` fires, even if `work` ignores it. */
function race<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(new DOMException('The request was cancelled.', 'AbortError'));
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => reject(new DOMException('The request was cancelled.', 'AbortError'));
    signal.addEventListener('abort', onAbort, { once: true });
    work.then(resolve, reject).finally(() => signal.removeEventListener('abort', onAbort));
  });
}

const isAbort = (cause: unknown) => cause instanceof DOMException && cause.name === 'AbortError';

async function readError(response: Response): Promise<ApiError> {
  let body: unknown = null;
  try {
    body = await response.json();
  } catch {
    // Not JSON (a proxy error page, say). Fall through to a generic message.
  }
  const error = (body as { error?: { code?: unknown; message?: unknown; field?: unknown; retry_after_s?: unknown } } | null)?.error;
  if (error && typeof error.code === 'string' && typeof error.message === 'string') {
    const { error: _ignored, ...extra } = body as Record<string, unknown>;
    return new ApiError(error.code, error.message, response.status, {
      field: typeof error.field === 'string' ? error.field : undefined,
      retryAfterS: typeof error.retry_after_s === 'number' ? error.retry_after_s : undefined,
      extra,
    });
  }
  const code = response.status === 401 ? 'unauthenticated' : response.status === 403 ? 'forbidden' : response.status === 404 ? 'not_found' : response.status >= 500 ? 'unavailable' : 'internal';
  return new ApiError(code, `Familiar answered with an unexpected error (HTTP ${response.status}). Try again in a moment.`, response.status);
}

export async function request<T>(method: Method, path: string, body?: unknown, options: Options = {}, attempt = 0): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  if (method !== 'GET') {
    const csrf = hooks.csrf();
    if (csrf) headers['X-Dot-CSRF'] = csrf;
  }

  const timer = deadline(method === 'GET' ? READ_TIMEOUT_MS : WRITE_TIMEOUT_MS, options.signal);
  const payload = body === undefined ? undefined : JSON.stringify(body);
  let response: Response;
  try {
    try {
      response = await race(
        transport.fetch(path, {
          method,
          headers,
          body: payload,
          credentials: 'same-origin',
          cache: 'no-store',
          // A change finishes even if the page closes right after sending it (an
          // undo countdown running out as you leave, say). Browsers cap these at 64 KB.
          keepalive: method !== 'GET' && (payload?.length ?? 0) < KEEPALIVE_MAX,
          signal: timer.signal,
        }),
        timer.signal,
      );
    } catch (cause) {
      if (timer.expired()) throw new ApiError('timeout', TIMEOUT_MESSAGE);
      if (isAbort(cause)) throw cause;
      hooks.reachable(false);
      throw new ApiError('network', OFFLINE_MESSAGE);
    }
    hooks.reachable(true);

    if (response.ok) {
      if (response.status === 204) return undefined as T;
      let parsed: unknown;
      try {
        parsed = await race(response.json(), timer.signal);
      } catch (cause) {
        if (timer.expired()) throw new ApiError('timeout', TIMEOUT_MESSAGE);
        if (isAbort(cause)) throw cause;
        throw new ApiError('bad_response', 'Familiar sent a reply the portal couldn’t read. Reload the page and try again.', response.status);
      }
      noticeRecoveryKey(parsed);
      return parsed as T;
    }
  } finally {
    timer.done();
  }

  const error = await readError(response);
  if (attempt === 0 && error.code === 'reauth_required') {
    const confirmed = await hooks.reauth();
    if (!confirmed) throw new ApiError('reauth_cancelled', 'Confirm your password to continue.', 403);
    return request<T>(method, path, body, options, 1);
  }
  if (attempt === 0 && error.code === 'csrf') {
    await hooks.refreshSession();
    return request<T>(method, path, body, options, 1);
  }
  if (error.code === 'unauthenticated' && !options.publicCall) hooks.unauthenticated();
  if (error.code === 'totp_setup_required') hooks.totpSetupRequired();
  throw error;
}

const get = <T>(path: string, options?: Options) => request<T>('GET', path, undefined, options);
const post = <T>(path: string, body?: unknown, options?: Options) => request<T>('POST', path, body ?? {}, options);
const put = <T>(path: string, body: unknown) => request<T>('PUT', path, body);
const patch = <T>(path: string, body: unknown) => request<T>('PATCH', path, body);
const del = <T>(path: string, body?: unknown) => request<T>('DELETE', path, body);

const enc = encodeURIComponent;
const mac = (id: string) => `/api/instances/${enc(id)}`;
type Empty = Record<string, never>;

export type UploadOptions = {
  signal?: AbortSignal;
  /** Called with 0 to 1 as the request body goes out. */
  onProgress?: (fraction: number) => void;
};

/** Sends an upload and resolves with the server's reply, like `fetch`. */
export type UploadSender = (path: string, file: Blob, headers: Record<string, string>, options: UploadOptions) => Promise<Response>;

/**
 * The browser's own sender. `fetch` can't report how much of a request body
 * has gone out, so this uses XMLHttpRequest and rebuilds a Response from it.
 * Rejects with a TypeError when the network fails and an AbortError when the
 * signal fires, the same as `fetch`.
 */
const sendWithXhr: UploadSender = (path, file, headers, { signal, onProgress }) =>
  new Promise((resolve, reject) => {
    const aborted = () => new DOMException('The upload was cancelled.', 'AbortError');
    if (signal?.aborted) return reject(aborted());
    const xhr = new XMLHttpRequest();
    xhr.open('POST', path);
    for (const [name, value] of Object.entries(headers)) xhr.setRequestHeader(name, value);
    xhr.responseType = 'blob';
    const onAbort = () => xhr.abort();
    signal?.addEventListener('abort', onAbort, { once: true });
    const done = () => signal?.removeEventListener('abort', onAbort);
    if (onProgress) {
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable && event.total > 0) onProgress(event.loaded / event.total);
      };
    }
    xhr.onload = () => {
      done();
      const nullBody = xhr.status === 204 || xhr.status === 205 || xhr.status === 304;
      const responseHeaders = new Headers();
      const type = xhr.getResponseHeader('Content-Type');
      if (type) responseHeaders.set('Content-Type', type);
      // A status Response can't hold (0 from some proxies) counts as a network failure.
      if (xhr.status < 200 || xhr.status > 599) return reject(new TypeError('Failed to fetch'));
      resolve(new Response(nullBody ? null : (xhr.response as Blob), { status: xhr.status, headers: responseHeaders }));
    };
    xhr.onerror = () => {
      done();
      reject(new TypeError('Failed to fetch'));
    };
    xhr.ontimeout = xhr.onerror;
    xhr.onabort = () => {
      done();
      reject(aborted());
    };
    xhr.send(file);
  });

/** How uploads leave the browser. The demo backend (dev only) swaps this out. */
export const uploads: { send: UploadSender } = { send: sendWithXhr };

/**
 * Upload raw bytes (a file dropped on the screen or the Files page). Kept off
 * `request` because the body is a Blob, not JSON, and because it reports
 * progress through `onProgress`.
 */
export async function upload(path: string, file: Blob, options: UploadOptions = {}, attempt = 0): Promise<FileItem> {
  const headers: Record<string, string> = { Accept: 'application/json', 'Content-Type': 'application/octet-stream' };
  const csrf = hooks.csrf();
  if (csrf) headers['X-Dot-CSRF'] = csrf;
  // No overall limit (files run to 2 GB); it fails only when the bytes stop moving.
  const timer = deadline(TRANSFER_STALL_MS, options.signal);
  let response: Response;
  try {
    const onProgress = (fraction: number) => {
      timer.touch();
      options.onProgress?.(fraction);
    };
    try {
      response = await race(uploads.send(path, file, headers, { signal: timer.signal, onProgress }), timer.signal);
    } catch (cause) {
      if (timer.expired()) throw new ApiError('timeout', 'The upload stalled. Check your connection and try again.');
      if (isAbort(cause)) throw cause;
      hooks.reachable(false);
      throw new ApiError('network', OFFLINE_MESSAGE);
    }
    hooks.reachable(true);
    if (response.ok) return (await race(response.json(), timer.signal)) as FileItem;
  } finally {
    timer.done();
  }
  const error = await readError(response);
  if (attempt === 0 && error.code === 'csrf') {
    await hooks.refreshSession();
    return upload(path, file, options, 1);
  }
  if (error.code === 'unauthenticated') hooks.unauthenticated();
  if (error.code === 'totp_setup_required') hooks.totpSetupRequired();
  throw error;
}

export const api = {
  // Session and sign-in
  session: () => get<SessionInfo>('/api/session', { publicCall: true }),
  setup: (body: { token: string; username: string; display_name: string; password: string }) => post<SessionInfo & WithRecoveryKey>('/api/setup', body, { publicCall: true }),
  login: (body: { username: string; password: string }) => post<SessionInfo & WithRecoveryKey>('/api/login', body, { publicCall: true }),
  loginTotp: (body: { challenge: string; code: string }) => post<SessionInfo & WithRecoveryKey>('/api/login/totp', body, { publicCall: true }),
  logout: () => post<Empty>('/api/logout'),
  invite: (code: string) => get<InviteInfo>(`/api/invites/${enc(code)}`, { publicCall: true }),
  acceptInvite: (code: string, body: { username: string; display_name: string; password: string; recovery_key?: string; discard_encrypted_macs?: boolean }) =>
    post<SessionInfo & WithRecoveryKey>(`/api/invites/${enc(code)}/accept`, body, { publicCall: true }),

  // Account
  account: () => get<AccountInfo>('/api/account'),
  updateAccount: (body: { display_name: string }) => patch<User>('/api/account', body),
  changePassword: (body: { current: string; password: string }) => post<WithRecoveryKey>('/api/account/password', body),
  startTotp: () => post<{ secret: string; otpauth_url: string }>('/api/account/totp'),
  confirmTotp: (code: string) => post<User>('/api/account/totp/confirm', { code }),
  revokeSession: (id: string) => del<Empty>(`/api/account/sessions/${enc(id)}`),
  /** Also unlocks the person's encrypted Macs after a server restart. */
  confirm: (body: { password: string; code?: string }) => post<{ until: string } & WithRecoveryKey>('/api/account/confirm', body, { publicCall: true }),
  /** A new recovery key; the old one stops working. Needs re-auth. */
  newRecoveryKey: () => post<{ recovery_key: string }>('/api/account/recovery-key'),
  /** The person confirmed they saved their recovery key. */
  recoveryKeySaved: () => post<Empty>('/api/account/recovery-key/saved'),
  security: () => get<SecurityEvent[]>('/api/account/security'),

  // Instances
  instances: () => get<Instance[]>('/api/instances'),
  createInstance: (body: { name: string; driver: 'qemu' | 'remote'; cpus?: number; memory_mb?: number; disk_gb?: number; remote_host?: string; approval_mode?: 'ask' | 'changes' | 'all' }) =>
    post<{ instance: Instance; pairing?: string }>('/api/instances', body),
  instance: (id: string) => get<Instance>(mac(id)),
  updateInstance: (id: string, body: Partial<Pick<Instance, 'name' | 'paused' | 'cpus' | 'memory_mb' | 'sleep_after_minutes' | 'hibernate_after_minutes' | 'snapshot_keep_days' | 'snapshot_max_gb'>>) =>
    patch<Instance>(mac(id), body),
  deleteInstance: (id: string, confirm: string) => del<Empty>(mac(id), { confirm }),
  power: (id: string, action: PowerAction) => post<Instance>(`${mac(id)}/power`, { action }),
  /** Encrypt a shut-down Mac's disk; progress arrives as instance events. */
  encryptInstance: (id: string) => post<Instance>(`${mac(id)}/encrypt`),
  thumbnailUrl: (id: string) => `${mac(id)}/thumbnail`,
  eventsUrl: (id: string) => `${mac(id)}/events`,

  // Permissions and approvals
  permissions: (id: string) => get<PermissionGroup[]>(`${mac(id)}/permissions`),
  setPermission: (id: string, group: string, mode: PermissionMode) => put<PermissionGroup>(`${mac(id)}/permissions/${enc(group)}`, { mode }),
  /** Asks the Mac which apps have MCP servers (starting none) and returns every permission group. */
  findAppServers: (id: string) => post<PermissionGroup[]>(`${mac(id)}/app-servers/refresh`),
  approvals: (id: string) => get<Approval[]>(`${mac(id)}/approvals`),
  decide: (id: string, approval: string, decision: 'approve' | 'deny') => post<Approval>(`${mac(id)}/approvals/${enc(approval)}`, { decision }),
  approvalMode: (id: string) => get<{ mode: ApprovalMode }>(`${mac(id)}/permissions/mode`),
  setApprovalMode: (id: string, mode: Exclude<ApprovalMode, 'custom'>) => put<{ mode: ApprovalMode; groups: PermissionGroup[] }>(`${mac(id)}/permissions/mode`, { mode }),

  // Review: changes made without asking
  review: (id: string, status: 'pending' | 'done') => get<ReviewItem[]>(`${mac(id)}/review?status=${status}`),
  decideReview: (id: string, item: string, decision: 'accept' | 'decline', note?: string) => post<ReviewItem>(`${mac(id)}/review/${enc(item)}`, note ? { decision, note } : { decision }),
  acceptAllReview: (id: string, scope: { session?: string; ids?: string[] } = {}) => post<{ accepted: number }>(`${mac(id)}/review/accept-all`, scope),

  // Activity
  activity: (id: string, query: { before?: string; limit?: number; outcome?: 'error' } = {}) => {
    const params = new URLSearchParams();
    if (query.before) params.set('before', query.before);
    if (query.limit) params.set('limit', String(query.limit));
    if (query.outcome) params.set('outcome', query.outcome);
    const qs = params.toString();
    return get<ActivityPage>(`${mac(id)}/activity${qs ? `?${qs}` : ''}`);
  },

  // Apps
  apps: (id: string) => get<AppEntry[]>(`${mac(id)}/apps`),
  setApp: (id: string, app: string, enabled: boolean) => put<AppEntry>(`${mac(id)}/apps/${enc(app)}`, { enabled }),
  /** 202 `{ installing: true }` when the install outlasts the request (about 45 s). */
  installApp: (id: string, source: InstallSource) => post<AppEntry | { installing: true }>(`${mac(id)}/apps/install`, { source }),
  addCustomPack: (id: string, manifest: string) => post<AppEntry>(`${mac(id)}/apps/custom`, { manifest }),
  removeCustomPack: (id: string, app: string) => del<Empty>(`${mac(id)}/apps/custom/${enc(app)}`),

  // Snapshots
  snapshots: (id: string) => get<Snapshot[]>(`${mac(id)}/snapshots`),
  createSnapshot: (id: string, name: string) => post<Snapshot>(`${mac(id)}/snapshots`, { name }),
  restoreSnapshot: (id: string, sid: string) => post<Instance>(`${mac(id)}/snapshots/${enc(sid)}/restore`),
  deleteSnapshot: (id: string, sid: string) => del<Empty>(`${mac(id)}/snapshots/${enc(sid)}`),
  keepSnapshot: (id: string, sid: string, kept: boolean) => put<Snapshot>(`${mac(id)}/snapshots/${enc(sid)}`, { kept }),
  snapshotUsage: (id: string) => get<SnapshotUsage>(`${mac(id)}/snapshot-usage`),

  // Connections
  connections: (id: string) => get<Connections>(`${mac(id)}/connections`),
  createToken: (id: string, body: { name: string; expires_in_days?: number }) => post<{ token: AccessToken; secret: string }>(`${mac(id)}/tokens`, body),
  revokeToken: (id: string, tid: string) => del<Empty>(`${mac(id)}/tokens/${enc(tid)}`),
  revokeGrant: (id: string, gid: string) => del<Empty>(`${mac(id)}/grants/${enc(gid)}`),

  // Files, clipboard and links: the owner using their own Mac
  files: (id: string, path: string) => get<FolderListing>(`${mac(id)}/files?path=${enc(path)}`),
  downloadUrl: (id: string, path: string) => `${mac(id)}/files/download?path=${enc(path)}`,
  /** Into `folder`; a name that exists gets " 2" like Finder. */
  uploadFile: (id: string, folder: string, file: File, options?: UploadOptions) =>
    upload(`${mac(id)}/files/upload?path=${enc(folder)}&name=${enc(file.name)}`, file, options),
  createFolder: (id: string, path: string) => post<FileItem>(`${mac(id)}/files/folder`, { path }),
  trashFile: (id: string, path: string) => post<Empty>(`${mac(id)}/files/trash`, { path }),
  clipboard: (id: string) => get<{ text: string | null }>(`${mac(id)}/clipboard`),
  setClipboard: (id: string, text: string) => put<Empty>(`${mac(id)}/clipboard`, { text }),
  openOnMac: (id: string, url: string) => post<Empty>(`${mac(id)}/open`, { url }),

  // Storage
  storage: () => get<MyStorage>('/api/storage'),
  /** Throw away a hibernated Mac's saved memory; it starts fresh next time. */
  discardMemory: (id: string) => post<Instance>(`${mac(id)}/storage/discard-memory`),

  // Preferences
  preferences: () => get<Preferences>('/api/account/preferences'),
  setPreferences: (body: Partial<Preferences>) => put<Preferences>('/api/account/preferences', body),

  // Web Push (approval notifications with no tab open)
  pushKey: () => get<PushKey>('/api/push/key'),
  addPushSubscription: (body: PushSubscriptionBody) => post<Empty>('/api/push/subscriptions', body),
  removePushSubscription: (endpoint: string) => del<Empty>('/api/push/subscriptions', { endpoint }),

  // OAuth consent
  oauthRequest: (id: string) => get<OAuthRequest>(`/api/oauth/requests/${enc(id)}`),
  answerOAuth: (id: string, body: { instance_id: string; approve: boolean }) => post<{ redirect: string }>(`/api/oauth/requests/${enc(id)}`, body),

  // Admin
  overview: () => get<HostOverview & { require_totp?: boolean }>('/api/admin/overview'),
  users: () => get<AdminUser[]>('/api/admin/users'),
  updateUser: (id: string, body: { disabled?: boolean; role?: Role; max_instances?: number }) => patch<AdminUser>(`/api/admin/users/${enc(id)}`, body),
  deleteUser: (id: string, confirm: string) => del<Empty>(`/api/admin/users/${enc(id)}`, { confirm }),
  resetUser: (id: string) => post<{ invite_url: string }>(`/api/admin/users/${enc(id)}/reset`),
  adminInstances: () => get<AdminInstance[]>('/api/admin/instances'),
  adminStop: (id: string) => post<Empty>(`/api/admin/instances/${enc(id)}/stop`),
  invites: () => get<Invite[]>('/api/admin/invites'),
  createInvite: (body: { role: Role; expires_in_hours?: number }) => post<{ invite: Invite; url: string }>('/api/admin/invites', body),
  revokeInvite: (id: string) => del<Empty>(`/api/admin/invites/${enc(id)}`),
  images: () => get<ImageStatus>('/api/admin/images'),
  startBuilder: (macos: string) => post<ImageStatus>('/api/admin/images/builder', { macos }),
  promoteImage: () => post<ImageStatus>('/api/admin/images/promote'),
  discardBuilder: () => del<ImageStatus>('/api/admin/images/builder'),
  settings: (body: { require_totp?: boolean; memory_budget_mb?: number; max_running?: number }) => put<HostOverview & { require_totp?: boolean }>('/api/admin/settings', body),
  adminEventsUrl: () => '/api/admin/events',
  adminStorage: () => get<AdminStorage>('/api/admin/storage'),
  setStorageDefaults: (body: Partial<StorageDefaults & SnapshotCap>) => put<AdminStorage>('/api/admin/storage/defaults', body),
  setPersonStorage: (userId: string, body: Partial<StorageOverrides>) => put<AdminStorage>(`/api/admin/users/${enc(userId)}/storage`, body),

  // Updates
  updates: () => get<UpdatesStatus>('/api/admin/updates'),
  /** Any subset of `UpdatesStatus.settings`, sent flat. Needs re-auth. */
  setUpdates: (body: Partial<UpdatesStatus['settings']>) => put<UpdatesStatus>('/api/admin/updates', body),
  checkUpdates: () => post<UpdatesStatus>('/api/admin/updates/check'),
  /** Installs `available`; the server restarts within seconds. Needs re-auth. */
  installUpdate: () => post<UpdatesStatus>('/api/admin/updates/install'),
  rollbackUpdate: () => post<UpdatesStatus>('/api/admin/updates/rollback'),
  setAgentChannel: (id: string, channel: AgentChannel) => patch<Instance>(mac(id), { agent_channel: channel }),
  /** The Mac must be running. */
  updateAgent: (id: string) => post<Instance>(`${mac(id)}/agent/update`),
};
