import { describe, expect, it } from 'vitest';
import { parentPath, parseRoute, safeNext, safeRedirect } from './routes';

describe('parseRoute', () => {
  it.each([
    ['/', { name: 'macs' }],
    ['/setup', { name: 'setup' }],
    ['/signin', { name: 'signin' }],
    ['/invite/abc123', { name: 'invite', code: 'abc123' }],
    ['/macs/studio', { name: 'mac', id: 'studio', section: 'overview' }],
    ['/macs/studio/', { name: 'mac', id: 'studio', section: 'overview' }],
    ['/macs/st%20udio/apps', { name: 'mac', id: 'st udio', section: 'apps' }],
    ['/macs/studio/screen', { name: 'mac', id: 'studio', section: 'screen' }],
    ['/macs/studio/review', { name: 'mac', id: 'studio', section: 'review' }],
    ['/connect', { name: 'consent' }],
    ['/account', { name: 'account' }],
    ['/admin', { name: 'admin', section: 'overview' }],
    ['/admin/invites', { name: 'admin', section: 'invites' }],
    ['/admin/updates', { name: 'admin', section: 'updates' }],
    ['/admin/image/screen', { name: 'builder' }],
  ])('%s', (path, route) => expect(parseRoute(path)).toEqual(route));

  it.each(['/macs/studio/nope', '/admin/nope', '/macs/%E0%A4%A/apps', '/macs/a%2Fb', '/invite', '/elsewhere'])('rejects %s', (path) => {
    expect(parseRoute(path)).toEqual({ name: 'missing' });
  });

  it('knows where back goes on a phone', () => {
    expect(parentPath(parseRoute('/macs/m/apps'))).toBe('/macs/m');
    expect(parentPath(parseRoute('/macs/m'))).toBe('/');
    expect(parentPath(parseRoute('/admin/users'))).toBe('/admin');
  });
});

describe('safeNext', () => {
  it.each([
    ['/macs/m?x=1', '/macs/m?x=1'],
    [null, '/'],
    ['https://evil.example/', '/'],
    ['//evil.example/path', '/'],
    ['/\\evil.example', '/'],
    ['javascript:alert(1)', '/'],
  ])('%s becomes %s', (input, output) => expect(safeNext(input)).toBe(output));
});

describe('safeRedirect', () => {
  it('follows http and https only', () => {
    globalThis.location ??= { origin: 'https://home.example.test' } as Location;
    expect(safeRedirect('https://claude.ai/callback?code=1')).toBe('https://claude.ai/callback?code=1');
    expect(safeRedirect('javascript:alert(1)')).toBeNull();
    expect(safeRedirect('data:text/html,hi')).toBeNull();
  });
});
