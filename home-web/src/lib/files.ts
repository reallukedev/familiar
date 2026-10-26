// Pure helpers for the Files page: Finder's sorting, kinds, names and paths.
import type { FileItem } from './types';

export type SortKey = 'name' | 'modified' | 'size' | 'kind';
export type Sort = { key: SortKey; dir: 'asc' | 'desc' };

export type FileGlyph = 'folder' | 'doc' | 'text' | 'image' | 'video' | 'audio' | 'archive' | 'app' | 'code' | 'alias';
export type FileType = {
  glyph: FileGlyph;
  /** Finder's Kind column. */
  label: string;
  /** Opens as a folder in the list. */
  navigable: boolean;
  /** Can be downloaded as one file. */
  downloadable: boolean;
};

const collator = new Intl.Collator('en', { numeric: true, sensitivity: 'base' });

const TYPES: Record<string, [FileGlyph, string]> = {
  png: ['image', 'PNG image'], jpg: ['image', 'JPEG image'], jpeg: ['image', 'JPEG image'], heic: ['image', 'HEIC image'],
  gif: ['image', 'GIF image'], webp: ['image', 'WebP image'], tif: ['image', 'TIFF image'], tiff: ['image', 'TIFF image'],
  svg: ['image', 'SVG image'], bmp: ['image', 'BMP image'],
  pdf: ['doc', 'PDF document'], pages: ['doc', 'Pages document'], numbers: ['doc', 'Numbers spreadsheet'], key: ['doc', 'Keynote presentation'],
  doc: ['doc', 'Word document'], docx: ['doc', 'Word document'], xls: ['doc', 'Excel spreadsheet'], xlsx: ['doc', 'Excel spreadsheet'],
  ppt: ['doc', 'PowerPoint presentation'], pptx: ['doc', 'PowerPoint presentation'], csv: ['doc', 'CSV document'],
  txt: ['text', 'Plain Text'], md: ['text', 'Markdown document'], rtf: ['text', 'Rich Text document'], log: ['text', 'Log file'],
  json: ['code', 'JSON'], js: ['code', 'JavaScript'], ts: ['code', 'TypeScript'], tsx: ['code', 'TypeScript'], swift: ['code', 'Swift source'],
  py: ['code', 'Python script'], sh: ['code', 'Shell script'], html: ['code', 'HTML document'], css: ['code', 'CSS'], rs: ['code', 'Rust source'],
  yml: ['code', 'YAML'], yaml: ['code', 'YAML'], xml: ['code', 'XML document'], plist: ['code', 'Property List'],
  zip: ['archive', 'ZIP archive'], xip: ['archive', 'XIP archive'], dmg: ['archive', 'Disk Image'], pkg: ['archive', 'Installer package'],
  tar: ['archive', 'Tar archive'], gz: ['archive', 'Gzip archive'], tgz: ['archive', 'Gzip archive'], bz2: ['archive', 'Bzip2 archive'],
  '7z': ['archive', '7-Zip archive'], rar: ['archive', 'RAR archive'],
  mov: ['video', 'QuickTime movie'], mp4: ['video', 'MPEG-4 movie'], m4v: ['video', 'MPEG-4 movie'],
  mp3: ['audio', 'MP3 audio'], m4a: ['audio', 'Apple MPEG-4 audio'], wav: ['audio', 'WAV audio'], aiff: ['audio', 'AIFF audio'], flac: ['audio', 'FLAC audio'],
};

/** Folders macOS shows as one item (Finder opens them; it never lists inside). */
const PACKAGES: Record<string, [FileGlyph, string]> = {
  app: ['app', 'Application'],
  photoslibrary: ['image', 'Photos Library'],
  bundle: ['app', 'Bundle'],
  framework: ['app', 'Framework'],
  xcodeproj: ['code', 'Xcode project'],
  rtfd: ['text', 'Rich Text document'],
};

function extension(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toLowerCase() : '';
}

export function fileType(item: Pick<FileItem, 'name' | 'kind'>): FileType {
  const ext = extension(item.name);
  if (item.kind === 'dir') {
    const pack = PACKAGES[ext] ?? (TYPES[ext]?.[0] === 'doc' ? TYPES[ext] : undefined);
    if (pack) return { glyph: pack[0], label: pack[1], navigable: false, downloadable: false };
    return { glyph: 'folder', label: 'Folder', navigable: true, downloadable: false };
  }
  if (item.kind === 'symlink') return { glyph: 'alias', label: 'Alias', navigable: !ext, downloadable: !!ext };
  if (item.kind === 'other') return { glyph: 'doc', label: 'Item', navigable: false, downloadable: false };
  const known = TYPES[ext];
  if (known) return { glyph: known[0], label: known[1], navigable: false, downloadable: true };
  return { glyph: 'doc', label: ext ? `${ext.toUpperCase()} file` : 'Document', navigable: false, downloadable: true };
}

const FIRST_DIR: Record<SortKey, Sort['dir']> = { name: 'asc', kind: 'asc', modified: 'desc', size: 'desc' };

/** Clicking a column: the same one flips, a new one starts where Finder starts it. */
export function nextSort(current: Sort, key: SortKey): Sort {
  if (current.key === key) return { key, dir: current.dir === 'asc' ? 'desc' : 'asc' };
  return { key, dir: FIRST_DIR[key] };
}

const isFolder = (item: FileItem) => item.kind === 'dir' && fileType(item).navigable;

/** Folders on top, then the chosen column; ties fall back to the name. */
export function sortEntries(entries: readonly FileItem[], sort: Sort): FileItem[] {
  const sign = sort.dir === 'asc' ? 1 : -1;
  const byName = (a: FileItem, b: FileItem) => collator.compare(a.name, b.name);
  const time = (item: FileItem) => (item.modified ? Date.parse(item.modified) : NaN);
  return [...entries].sort((a, b) => {
    const group = Number(isFolder(b)) - Number(isFolder(a));
    if (group) return group;
    switch (sort.key) {
      case 'name':
        return sign * byName(a, b);
      case 'size':
        return sign * (a.size - b.size) || byName(a, b);
      case 'kind':
        return sign * collator.compare(fileType(a).label, fileType(b).label) || byName(a, b);
      case 'modified': {
        const [ta, tb] = [time(a), time(b)];
        if (Number.isNaN(ta) || Number.isNaN(tb)) return Number(Number.isNaN(ta)) - Number(Number.isNaN(tb)) || byName(a, b);
        return sign * (ta - tb) || byName(a, b);
      }
    }
  });
}

const segmenter = typeof Intl.Segmenter === 'function' ? new Intl.Segmenter('en', { granularity: 'grapheme' }) : null;
const graphemes = (text: string) => (segmenter ? Array.from(segmenter.segment(text), (part) => part.segment) : Array.from(text));

/**
 * Split a name so CSS can truncate the head and always show the tail, the
 * way Finder truncates in the middle and keeps the extension visible.
 */
export function splitMiddle(name: string, tail = 9): { head: string; tail: string } {
  const chars = graphemes(name);
  if (chars.length <= 18) return { head: name, tail: '' };
  const ext = extension(name);
  const want = Math.max(tail, ext && ext.length <= 8 ? ext.length + 6 : tail);
  const keep = Math.min(want, Math.floor(chars.length / 2) - 1);
  return { head: chars.slice(0, -keep).join(''), tail: chars.slice(-keep).join('') };
}

/** A path as the page keeps it in the URL: `~` for the home folder, no trailing slash. */
export function toParam(path: string, home: string | null): string {
  let clean = path.trim().replace(/\/{2,}/g, '/');
  if (!clean) return '~';
  if (clean.length > 1) clean = clean.replace(/\/+$/, '');
  if (home && clean === home) return '~';
  if (home && clean.startsWith(`${home}/`)) return `~${clean.slice(home.length)}`;
  return clean;
}

/** The home folder a listed path lives in, from macOS's `/Users/<name>` layout. */
export function homeOf(path: string): string | null {
  const match = /^\/Users\/([^/]+)/.exec(path);
  return match && match[1] !== 'Shared' ? `/Users/${match[1]}` : null;
}

export function parentOf(path: string): string | null {
  if (path === '~' || path === '/' || !path.includes('/')) return null;
  const cut = path.lastIndexOf('/');
  return cut === 0 ? '/' : path.slice(0, cut);
}

export function joinPath(folder: string, name: string): string {
  return folder === '/' ? `/${name}` : `${folder}/${name}`;
}

export function baseName(path: string): string {
  if (path === '~') return 'Home';
  if (path === '/') return 'Macintosh HD';
  return path.slice(path.lastIndexOf('/') + 1);
}

export type Crumb = { label: string; path: string };
export function crumbs(path: string): Crumb[] {
  const home = path === '~' || path.startsWith('~/');
  const root: Crumb = home ? { label: 'Home', path: '~' } : { label: 'Macintosh HD', path: '/' };
  const rest = (home ? path.slice(1) : path).split('/').filter(Boolean);
  const out = [root];
  for (const part of rest) out.push({ label: part, path: joinPath(out.at(-1)!.path, part) });
  return out;
}

function splitName(name: string): [string, string] {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ''];
}

/** Finder's numbering for a name that's taken: "untitled folder 2", "report 2.pdf". */
export function uniqueName(existing: Iterable<string>, wanted: string): string {
  const taken = new Set(Array.from(existing, (name) => name.toLowerCase()));
  if (!taken.has(wanted.toLowerCase())) return wanted;
  const [stem, ext] = splitName(wanted);
  for (let n = 2; ; n += 1) {
    const candidate = `${stem} ${n}${ext}`;
    if (!taken.has(candidate.toLowerCase())) return candidate;
  }
}

/** Why a Mac can't use this name for a new item here, or null when it can. */
export function nameError(raw: string, existing: Iterable<string> = []): string | null {
  const name = raw.trim();
  if (!name) return 'Enter a name.';
  if (name.includes('/') || name.includes('\0')) return 'Names on a Mac can’t contain “/”.';
  if (name === '.' || name === '..') return 'A name can’t be “.” or “..”.';
  if (name.startsWith('.')) return 'Names starting with “.” are hidden on a Mac. Choose a name without the dot.';
  if (new TextEncoder().encode(name).length > 255) return 'That name is too long for a Mac. Use a shorter one.';
  const lower = name.toLowerCase();
  for (const other of existing) if (other.toLowerCase() === lower) return `“${name}” already exists here. Choose a different name.`;
  return null;
}

const fold = (text: string) => text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

/**
 * Type-to-select: the first item whose name starts with what was typed, or
 * the nearest one after it alphabetically, as Finder does.
 */
export function typeSelect(items: readonly FileItem[], typed: string): number | null {
  if (!items.length) return null;
  const query = fold(typed);
  const hit = items.findIndex((item) => fold(item.name).startsWith(query));
  if (hit >= 0) return hit;
  const after = items.findIndex((item) => collator.compare(item.name, typed) > 0);
  return after >= 0 ? after : items.length - 1;
}

const clock = new Intl.DateTimeFormat('en', { hour: 'numeric', minute: '2-digit' });
const fullDate = new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', year: 'numeric' });

/** Finder's Date Modified: "Today at 9:41 AM", "Mar 2, 2026 at 8:00 AM". */
export function finderDate(iso: string | null, now = Date.now()): string {
  if (!iso) return '--';
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return '--';
  const today = new Date(now);
  const yesterday = new Date(now - 86_400_000);
  const day = at.toDateString() === today.toDateString() ? 'Today' : at.toDateString() === yesterday.toDateString() ? 'Yesterday' : fullDate.format(at);
  return `${day} at ${clock.format(at).replace(/\u202f/g, ' ')}`;
}
