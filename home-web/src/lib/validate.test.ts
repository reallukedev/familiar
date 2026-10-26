import { describe, expect, it } from 'vitest';
import { appStoreId, appStoreIdError, caskError, downloadUrlError, passwordError, sha256Error, usernameError } from './validate';

describe('validate', () => {
  it('follows the server’s username and password rules', () => {
    expect(usernameError('jo.park')).toBeNull();
    expect(usernameError('J')).not.toBeNull();
    expect(usernameError('Jo Park')).not.toBeNull();
    expect(passwordError('123456789')).not.toBeNull();
    expect(passwordError('1234567890')).toBeNull();
  });

  it('accepts App Store links, ids and bare numbers', () => {
    expect(appStoreId('https://apps.apple.com/us/app/xcode/id497799835?mt=12')).toBe('497799835');
    expect(appStoreId('id497799835')).toBe('497799835');
    expect(appStoreIdError(appStoreId('497799835'))).toBeNull();
    expect(appStoreIdError(appStoreId('xcode'))).not.toBeNull();
  });

  it('only installs https disk images and archives', () => {
    expect(downloadUrlError('https://example.com/App.dmg')).toBeNull();
    expect(downloadUrlError('http://example.com/App.dmg')).toMatch(/https/);
    expect(downloadUrlError('https://example.com/App.exe')).toMatch(/\.dmg/);
    expect(sha256Error('a'.repeat(64))).toBeNull();
    expect(sha256Error('abc')).not.toBeNull();
    expect(caskError('visual-studio-code')).toBeNull();
    expect(caskError('Not A Cask')).not.toBeNull();
  });
});
