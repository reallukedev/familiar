import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, api, hooks, request } from './api';
import { transport } from './transport';

type Call = { url: string; init: RequestInit };
let calls: Call[];

function respond(...responses: (Response | Error)[]) {
  calls = [];
  const queue = [...responses];
  transport.fetch = async (url, init) => {
    calls.push({ url, init });
    const next = queue.shift();
    if (!next) throw new Error('unexpected extra request');
    if (next instanceof Error) throw next;
    return next;
  };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
const failure = (status: number, code: string, message: string) => json({ error: { code, message } }, status);

const original = { ...hooks };
beforeEach(() => {
  hooks.csrf = () => 'token-123';
});
afterEach(() => Object.assign(hooks, original));

describe('request', () => {
  it('sends the CSRF header on changes but not on reads', async () => {
    respond(json([]), json({}));
    await api.instances();
    await api.logout();
    expect(new Headers(calls[0]!.init.headers).get('X-Dot-CSRF')).toBeNull();
    expect(new Headers(calls[1]!.init.headers).get('X-Dot-CSRF')).toBe('token-123');
    expect(calls[1]!.init.credentials).toBe('same-origin');
  });

  it('turns the server error body into a typed ApiError with field, retry and extras', async () => {
    respond(json({ error: { code: 'rate_limited', message: 'Too many tries.', field: 'password', retry_after_s: 42 }, challenge: 'abc' }, 429));
    const error = await request('POST', '/api/login', {}, { publicCall: true }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ code: 'rate_limited', message: 'Too many tries.', field: 'password', retryAfterS: 42, status: 429 });
    expect((error as ApiError).extra.challenge).toBe('abc');
  });

  it('hands a one-time recovery key in any reply to the portal, and nothing else', async () => {
    const seen: string[] = [];
    hooks.recoveryKey = (key) => seen.push(key);
    respond(json({ user: null, recovery_key: 'ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567' }), json({ until: 'x' }), json({ recovery_key: 42 }));
    const session = await api.login({ username: 'alex', password: 'pw' });
    await api.confirm({ password: 'pw' });
    await api.confirm({ password: 'pw' });
    expect(session.recovery_key).toBe('ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567');
    expect(seen).toEqual(['ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567']);
  });

  it('gives a readable error when a proxy answers with HTML', async () => {
    respond(new Response('<html>Bad gateway</html>', { status: 502 }));
    const error = (await api.instances().catch((e: unknown) => e)) as ApiError;
    expect(error.code).toBe('unavailable');
    expect(error.message).toMatch(/HTTP 502/);
  });

  it('asks for the password on reauth_required and retries the request once', async () => {
    const reauth = vi.fn(async () => true);
    hooks.reauth = reauth;
    respond(failure(403, 'reauth_required', 'Confirm your password.'), json({ token: { id: 't' }, secret: 's' }));
    const result = await api.createToken('mac', { name: 'CLI' });
    expect(reauth).toHaveBeenCalledOnce();
    expect(result.secret).toBe('s');
    expect(calls).toHaveLength(2);
    expect(calls[1]!.init.body).toBe(calls[0]!.init.body);
  });

  it('stops with reauth_cancelled when the person cancels the confirmation', async () => {
    hooks.reauth = async () => false;
    respond(failure(403, 'reauth_required', 'Confirm your password.'));
    const error = (await api.deleteInstance('mac', 'Mac').catch((e: unknown) => e)) as ApiError;
    expect(error.code).toBe('reauth_cancelled');
    expect(error.silent).toBe(true);
    expect(calls).toHaveLength(1);
  });

  it('does not loop when the retried request still needs confirmation', async () => {
    const reauth = vi.fn(async () => true);
    hooks.reauth = reauth;
    respond(failure(403, 'reauth_required', 'Confirm.'), failure(403, 'reauth_required', 'Confirm.'));
    const error = (await api.deleteInstance('mac', 'Mac').catch((e: unknown) => e)) as ApiError;
    expect(error.code).toBe('reauth_required');
    expect(reauth).toHaveBeenCalledOnce();
  });

  it('refreshes the session after a CSRF rejection and retries with the new token', async () => {
    let token = 'old';
    hooks.csrf = () => token;
    hooks.refreshSession = async () => {
      token = 'new';
    };
    respond(failure(403, 'csrf', 'Out of date.'), json({}));
    await api.logout();
    expect(new Headers(calls[1]!.init.headers).get('X-Dot-CSRF')).toBe('new');
  });

  it('reports an expired session, except for sign-in calls where 401 is an answer', async () => {
    const unauthenticated = vi.fn();
    hooks.unauthenticated = unauthenticated;
    respond(failure(401, 'unauthenticated', 'Sign in.'), failure(401, 'unauthenticated', 'Wrong password.'));
    await api.instances().catch(() => {});
    await api.login({ username: 'a', password: 'b' }).catch(() => {});
    expect(unauthenticated).toHaveBeenCalledOnce();
  });

  it('sends the person to two-factor enrollment when the server requires it', async () => {
    const gate = vi.fn();
    hooks.totpSetupRequired = gate;
    respond(failure(403, 'totp_setup_required', 'Turn on two-factor.'));
    const error = (await api.instances().catch((e: unknown) => e)) as ApiError;
    expect(gate).toHaveBeenCalledOnce();
    expect(error.silent).toBe(true);
  });

  it('marks the server unreachable on a network failure and reachable again on the next answer', async () => {
    const reachable = vi.fn();
    hooks.reachable = reachable;
    respond(new TypeError('Failed to fetch'), json([]));
    const error = (await api.instances().catch((e: unknown) => e)) as ApiError;
    expect(error.code).toBe('network');
    await api.instances();
    expect(reachable.mock.calls).toEqual([[false], [true]]);
  });

  it('encodes ids in paths', async () => {
    respond(json({}));
    await api.instance('a/b c');
    expect(calls[0]!.url).toBe('/api/instances/a%2Fb%20c');
  });
});

describe('upload', () => {
  it('refreshes a stale CSRF token and sends the file again', async () => {
    const { upload, uploads } = await import('./api');
    const original = uploads.send;
    const sent: string[] = [];
    let token = 'old';
    hooks.csrf = () => token;
    hooks.refreshSession = async () => {
      token = 'new';
    };
    uploads.send = async (_path, _file, headers) => {
      sent.push(headers['X-Dot-CSRF']!);
      return sent.length === 1 ? failure(403, 'csrf', 'Stale token') : json({ name: 'a.txt', path: '~/a.txt' });
    };
    try {
      const saved = await upload('/api/x', new Blob(['hi']));
      expect(saved.name).toBe('a.txt');
      expect(sent).toEqual(['old', 'new']);
    } finally {
      uploads.send = original;
    }
  });
});

describe('keepalive', () => {
  it('lets changes outlive the page but not reads or large bodies', async () => {
    respond(json([]), json({}), json({}));
    await api.instances();
    await api.logout();
    await request('POST', '/api/big', { blob: 'x'.repeat(70_000) });
    expect(calls[0]!.init.keepalive).toBe(false);
    expect(calls[1]!.init.keepalive).toBe(true);
    expect(calls[2]!.init.keepalive).toBe(false);
  });
});
