import { describe, expect, it } from 'vitest';
import {
  crumbs, fileType, finderDate, joinPath, nameError, nextSort, parentOf, splitMiddle, sortEntries, toParam, typeSelect, uniqueName,
} from './files';
import type { FileItem } from './types';

const f = (name: string, size = 0, modified: string | null = '2026-10-01T10:00:00Z'): FileItem => ({ name, path: `/Users/dot/${name}`, kind: 'file', size, modified });
const d = (name: string, modified: string | null = '2026-10-01T10:00:00Z'): FileItem => ({ name, path: `/Users/dot/${name}`, kind: 'dir', size: 0, modified });
const names = (items: FileItem[]) => items.map((item) => item.name);

describe('sortEntries', () => {
  it('sorts names the way Finder does: numbers by value, case ignored, folders on top', () => {
    const list = [f('file 10.txt'), d('zeta'), f('File 2.txt'), f('alpha.txt'), d('Beta')];
    expect(names(sortEntries(list, { key: 'name', dir: 'asc' }))).toEqual(['Beta', 'zeta', 'alpha.txt', 'File 2.txt', 'file 10.txt']);
    expect(names(sortEntries(list, { key: 'name', dir: 'desc' }))).toEqual(['zeta', 'Beta', 'file 10.txt', 'File 2.txt', 'alpha.txt']);
  });

  it('puts the largest file first by size and breaks ties by name', () => {
    const list = [f('b.zip', 10), f('a.zip', 10), f('huge.xip', 3_900_000_000), d('Folder'), f('tiny', 1)];
    expect(names(sortEntries(list, { key: 'size', dir: 'desc' }))).toEqual(['Folder', 'huge.xip', 'a.zip', 'b.zip', 'tiny']);
  });

  it('sorts by date with unknown dates last either way', () => {
    const list = [f('old', 0, '2020-01-01T00:00:00Z'), f('none', 0, null), f('new', 0, '2026-10-19T00:00:00Z')];
    expect(names(sortEntries(list, { key: 'modified', dir: 'desc' }))).toEqual(['new', 'old', 'none']);
    expect(names(sortEntries(list, { key: 'modified', dir: 'asc' }))).toEqual(['old', 'new', 'none']);
  });

  it('keeps app bundles with files, not folders, since they do not open as folders', () => {
    const list = [d('Sketchpad.app'), d('Notes'), f('a.txt')];
    expect(names(sortEntries(list, { key: 'name', dir: 'asc' }))).toEqual(['Notes', 'a.txt', 'Sketchpad.app']);
  });
});

describe('nextSort', () => {
  it('flips the direction on the same column and uses Finder’s first direction on a new one', () => {
    expect(nextSort({ key: 'name', dir: 'asc' }, 'name')).toEqual({ key: 'name', dir: 'desc' });
    expect(nextSort({ key: 'name', dir: 'asc' }, 'size')).toEqual({ key: 'size', dir: 'desc' });
    expect(nextSort({ key: 'size', dir: 'desc' }, 'modified')).toEqual({ key: 'modified', dir: 'desc' });
    expect(nextSort({ key: 'size', dir: 'desc' }, 'kind')).toEqual({ key: 'kind', dir: 'asc' });
  });
});

describe('fileType', () => {
  it('names kinds from the extension, ignoring case', () => {
    expect(fileType(f('Shot.PNG'))).toMatchObject({ glyph: 'image', label: 'PNG image' });
    expect(fileType(f('invoice.pdf'))).toMatchObject({ glyph: 'doc', label: 'PDF document' });
    expect(fileType(f('Xcode_17.xip'))).toMatchObject({ glyph: 'archive' });
    expect(fileType(f('backup.tar.gz'))).toMatchObject({ glyph: 'archive', label: 'Gzip archive' });
  });

  it('treats a folder as navigable and an app bundle as an application that is neither', () => {
    expect(fileType(d('Documents'))).toMatchObject({ glyph: 'folder', label: 'Folder', navigable: true, downloadable: false });
    expect(fileType(d('Sketchpad.app'))).toMatchObject({ glyph: 'app', label: 'Application', navigable: false, downloadable: false });
    expect(fileType(d('Photos Library.photoslibrary'))).toMatchObject({ navigable: false, downloadable: false });
  });

  it('falls back to a plain document for names without a known extension', () => {
    expect(fileType(f('README'))).toMatchObject({ glyph: 'doc', label: 'Document', downloadable: true });
    expect(fileType(f('.env'))).toMatchObject({ label: 'Document' });
    expect(fileType(f('weird.zzzz'))).toMatchObject({ label: 'ZZZZ file' });
  });

  it('opens a link without an extension like a folder and downloads one with an extension', () => {
    const link = (name: string): FileItem => ({ ...f(name), kind: 'symlink' });
    expect(fileType(link('Shared'))).toMatchObject({ glyph: 'alias', navigable: true });
    expect(fileType(link('notes.txt'))).toMatchObject({ glyph: 'alias', navigable: false, downloadable: true });
  });
});

describe('splitMiddle', () => {
  it('leaves short names whole', () => {
    expect(splitMiddle('notes.md')).toEqual({ head: 'notes.md', tail: '' });
  });

  it('keeps the end of a long name, extension included, so it survives truncation', () => {
    const name = 'Screenshot 2026-10-18 at 9.41.12 AM.png';
    const { head, tail } = splitMiddle(name);
    expect(head + tail).toBe(name);
    expect(tail.endsWith('.png')).toBe(true);
    expect(tail.length).toBeGreaterThanOrEqual(8);
    expect(tail.length).toBeLessThan(name.length / 2);
  });

  it('never splits a surrogate pair', () => {
    const name = 'Holiday photos from the beach 🏖️🏖️🏖️.jpeg';
    const { head, tail } = splitMiddle(name, 10);
    expect(head + tail).toBe(name);
    expect(/[\uD800-\uDBFF]$/.test(head)).toBe(false);
  });
});

describe('paths', () => {
  it('writes paths in the home folder with a tilde', () => {
    expect(toParam('/Users/dot', '/Users/dot')).toBe('~');
    expect(toParam('/Users/dot/Documents/Projects', '/Users/dot')).toBe('~/Documents/Projects');
    expect(toParam('/Users/dotty/Documents', '/Users/dot')).toBe('/Users/dotty/Documents');
    expect(toParam('/Applications/', null)).toBe('/Applications');
    expect(toParam('~/Documents//', null)).toBe('~/Documents');
    expect(toParam('  ', null)).toBe('~');
  });

  it('finds the parent of tilde and absolute paths', () => {
    expect(parentOf('~/Documents/Projects')).toBe('~/Documents');
    expect(parentOf('~/Documents')).toBe('~');
    expect(parentOf('~')).toBeNull();
    expect(parentOf('/Users')).toBe('/');
    expect(parentOf('/')).toBeNull();
  });

  it('joins a name onto a folder', () => {
    expect(joinPath('~', 'Downloads')).toBe('~/Downloads');
    expect(joinPath('/', 'Users')).toBe('/Users');
    expect(joinPath('~/a', 'b c')).toBe('~/a/b c');
  });

  it('builds the path bar from Home, or from the disk outside it', () => {
    expect(crumbs('~/Documents/Projects')).toEqual([
      { label: 'Home', path: '~' },
      { label: 'Documents', path: '~/Documents' },
      { label: 'Projects', path: '~/Documents/Projects' },
    ]);
    expect(crumbs('/Users')).toEqual([
      { label: 'Macintosh HD', path: '/' },
      { label: 'Users', path: '/Users' },
    ]);
  });
});

describe('uniqueName', () => {
  it('numbers a taken name like Finder, before the extension', () => {
    expect(uniqueName(['a'], 'untitled folder')).toBe('untitled folder');
    expect(uniqueName(['untitled folder', 'untitled folder 2'], 'untitled folder')).toBe('untitled folder 3');
    expect(uniqueName(['report.pdf'], 'report.pdf')).toBe('report 2.pdf');
  });

  it('compares names without case, as the Mac’s disk does', () => {
    expect(uniqueName(['Untitled Folder'], 'untitled folder')).toBe('untitled folder 2');
  });
});

describe('nameError', () => {
  it('accepts ordinary names', () => {
    expect(nameError('Taxes 2026')).toBeNull();
  });

  it('explains names a Mac can’t use', () => {
    expect(nameError('   ')).toMatch(/name/i);
    expect(nameError('a/b')).toMatch(/\//);
    expect(nameError('..')).toMatch(/\.\./);
    expect(nameError('x'.repeat(256))).toMatch(/long/i);
    expect(nameError('.secret')).toMatch(/hidden/);
  });

  it('refuses a name already in the folder', () => {
    expect(nameError('Notes', ['notes'])).toMatch(/already/);
  });
});

describe('typeSelect', () => {
  const list = sortEntries([d('Desktop'), d('Documents'), d('Downloads'), f('apple.txt'), f('Ärger.txt'), f('zebra.txt')], { key: 'name', dir: 'asc' });
  it('finds the first name that starts with what was typed', () => {
    expect(list[typeSelect(list, 'do')!]!.name).toBe('Documents');
    expect(list[typeSelect(list, 'DOW')!]!.name).toBe('Downloads');
    expect(list[typeSelect(list, 'ar')!]!.name).toBe('Ärger.txt');
  });

  it('lands on the nearest following name when nothing matches, as Finder does', () => {
    expect(list[typeSelect(list, 'm')!]!.name).toBe('zebra.txt');
    expect(typeSelect([], 'a')).toBeNull();
  });
});

describe('finderDate', () => {
  const now = new Date('2026-10-20T15:00:00').getTime();
  it('says Today and Yesterday with the time', () => {
    expect(finderDate(new Date('2026-10-20T09:41:00').toISOString(), now)).toBe('Today at 9:41 AM');
    expect(finderDate(new Date('2026-10-19T21:05:00').toISOString(), now)).toBe('Yesterday at 9:05 PM');
  });
  it('shows the full date otherwise, and dashes when unknown', () => {
    expect(finderDate(new Date('2026-03-02T08:00:00').toISOString(), now)).toBe('Mar 2, 2026 at 8:00 AM');
    expect(finderDate(null, now)).toBe('--');
  });
});
