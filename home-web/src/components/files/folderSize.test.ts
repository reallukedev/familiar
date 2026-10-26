import { describe, expect, it } from 'vitest';
import type { FileItem, FolderListing } from '../../lib/types';
import { walkFolder } from './folderSize';
import { looksLike, previewKind } from './preview';

const file = (path: string, size: number, kind: FileItem['kind'] = 'file'): FileItem => ({ name: path.split('/').pop()!, path, kind, size, modified: null });

function disk(tree: Record<string, FileItem[]>, truncated: string[] = []) {
  const calls: string[] = [];
  let active = 0;
  let peak = 0;
  const list = async (path: string): Promise<FolderListing> => {
    calls.push(path);
    active += 1;
    peak = Math.max(peak, active);
    await new Promise((resolve) => setTimeout(resolve, 1));
    active -= 1;
    const entries = tree[path];
    if (!entries) throw new Error(`no folder ${path}`);
    return { path, parent: null, entries, truncated: truncated.includes(path) };
  };
  return { list, calls, peak: () => peak };
}

describe('walkFolder', () => {
  it('totals everything inside, never following symlinks', async () => {
    const { list, calls } = disk({
      '/a': [file('/a/b', 0, 'dir'), file('/a/x.txt', 10), file('/a/link', 5, 'symlink')],
      '/a/b': [file('/a/b/y.png', 100), file('/a/b/c', 0, 'dir')],
      '/a/b/c': [file('/a/b/c/z', 1000)],
    });
    expect(await walkFolder('/a', list)).toEqual({ bytes: 1115, items: 6, atLeast: false });
    expect(calls).not.toContain('/a/link');
  });

  it('runs at most four listings at once', async () => {
    const children = Array.from({ length: 12 }, (_, i) => file(`/r/${i}`, 0, 'dir'));
    const tree: Record<string, FileItem[]> = { '/r': children };
    for (const child of children) tree[child.path] = [file(`${child.path}/f`, 1)];
    const { list, peak } = disk(tree);
    expect(await walkFolder('/r', list)).toEqual({ bytes: 12, items: 24, atLeast: false });
    expect(peak()).toBeLessThanOrEqual(4);
  });

  it('says "at least" at the cap, for cut-off listings and for unreadable folders', async () => {
    const many = Array.from({ length: 30 }, (_, i) => file(`/m/${i}`, 1));
    expect(await walkFolder('/m', disk({ '/m': many }).list, { cap: 10 })).toEqual({ bytes: 10, items: 10, atLeast: true });
    expect((await walkFolder('/m', disk({ '/m': many }, ['/m']).list)).atLeast).toBe(true);
    const locked = disk({ '/l': [file('/l/private', 0, 'dir'), file('/l/a', 3)] });
    expect(await walkFolder('/l', locked.list)).toEqual({ bytes: 3, items: 2, atLeast: true });
  });

  it('fails when the folder itself can’t be read, and stops when cancelled', async () => {
    await expect(walkFolder('/nope', disk({}).list)).rejects.toThrow('no folder');
    const abort = new AbortController();
    const walk = walkFolder('/a', disk({ '/a': [file('/a/x', 1)] }).list, { signal: abort.signal });
    abort.abort();
    await expect(walk).rejects.toThrow('Cancelled');
  });
});

describe('previews', () => {
  it('knows what it can draw from the name', () => {
    expect(previewKind({ name: 'a.PNG', kind: 'file' })).toBe('image');
    expect(previewKind({ name: 'notes.md', kind: 'file' })).toBe('text');
    expect(previewKind({ name: 'package.json', kind: 'file' })).toBe('code');
    expect(previewKind({ name: 'Projects', kind: 'dir' })).toBe('none');
    expect(previewKind({ name: 'Xcode.xip', kind: 'file' })).toBe('none');
  });

  it('checks the bytes match the name', () => {
    const bytes = (text: string) => new TextEncoder().encode(text);
    expect(looksLike('pdf', bytes('%PDF-1.7'))).toBe(true);
    expect(looksLike('pdf', bytes('Synthetic demo file'))).toBe(false);
    expect(looksLike('image', new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBe(true);
    expect(looksLike('image', bytes('<svg xmlns="http://www.w3.org/2000/svg"/>'))).toBe(true);
    expect(looksLike('text', new Uint8Array([0x62, 0x70, 0x00, 0x6c]))).toBe(false);
  });
});
