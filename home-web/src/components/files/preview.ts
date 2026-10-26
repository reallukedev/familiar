import { useEffect, useState } from 'react';
import { ApiError, api } from '../../lib/api';
import { transport } from '../../lib/transport';
import type { FileItem } from '../../lib/types';

// What Quick Look can draw for a file, and the loader that fetches it.

export type PreviewKind = 'image' | 'pdf' | 'text' | 'code' | 'video' | 'audio' | 'none';

const KINDS: Record<string, PreviewKind> = {};
const add = (kind: PreviewKind, exts: string) => exts.split(' ').forEach((ext) => (KINDS[ext] = kind));
add('image', 'png jpg jpeg gif webp svg bmp heic heif tif tiff avif ico');
add('pdf', 'pdf');
add('text', 'txt md markdown log csv tsv text');
add('code', 'json js mjs cjs jsx ts tsx swift py rb go rs c h cc cpp m sh zsh bash fish html htm css scss xml yml yaml toml ini conf cfg env plist sql lua java kt php');
add('video', 'mov mp4 m4v webm');
add('audio', 'mp3 m4a aac wav aiff aif flac ogg opus');

const MIME: Record<string, string> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
  bmp: 'image/bmp', heic: 'image/heic', heif: 'image/heif', tif: 'image/tiff', tiff: 'image/tiff', avif: 'image/avif', ico: 'image/x-icon',
  pdf: 'application/pdf',
  mov: 'video/quicktime', mp4: 'video/mp4', m4v: 'video/mp4', webm: 'video/webm',
  mp3: 'audio/mpeg', m4a: 'audio/mp4', aac: 'audio/aac', wav: 'audio/wav', aiff: 'audio/aiff', aif: 'audio/aiff', flac: 'audio/flac', ogg: 'audio/ogg', opus: 'audio/ogg',
};

/** The first part of a text file that Quick Look reads. */
export const TEXT_LIMIT = 200_000;
/** Bigger than this and Quick Look offers the download instead of loading it into the page. */
const BLOB_LIMIT: Partial<Record<PreviewKind, number>> = { image: 50_000_000, pdf: 100_000_000, video: 200_000_000, audio: 200_000_000 };

function extension(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toLowerCase() : '';
}

export function previewKind(item: Pick<FileItem, 'name' | 'kind'>): PreviewKind {
  if (item.kind !== 'file' && item.kind !== 'symlink') return 'none';
  return KINDS[extension(item.name)] ?? 'none';
}

const ascii = (bytes: Uint8Array, at: number, text: string) => Array.from(text).every((ch, i) => bytes[at + i] === ch.charCodeAt(0));

/**
 * Whether the bytes really are the kind the name says. A file that isn't
 * (a renamed file, or the demo's stand-in text) gets the info card, not a
 * broken image.
 */
export function looksLike(kind: PreviewKind, bytes: Uint8Array): boolean {
  switch (kind) {
    case 'pdf':
      return ascii(bytes, 0, '%PDF');
    case 'image': {
      if (ascii(bytes, 0, '\x89PNG') || (bytes[0] === 0xff && bytes[1] === 0xd8) || ascii(bytes, 0, 'GIF8') || ascii(bytes, 0, 'BM')) return true;
      if (ascii(bytes, 0, 'RIFF') && ascii(bytes, 8, 'WEBP')) return true;
      if (ascii(bytes, 0, 'II*\0') || ascii(bytes, 0, 'MM\0*') || ascii(bytes, 4, 'ftyp') || ascii(bytes, 0, '\0\0\x01\0')) return true;
      const head = new TextDecoder().decode(bytes.subarray(0, 512)).trimStart();
      return head.startsWith('<svg') || (head.startsWith('<?xml') && head.includes('<svg'));
    }
    case 'video':
      return ascii(bytes, 4, 'ftyp') || ascii(bytes, 4, 'moov') || ascii(bytes, 4, 'wide') || ascii(bytes, 4, 'mdat') || (bytes[0] === 0x1a && bytes[1] === 0x45);
    case 'audio':
      return (
        ascii(bytes, 0, 'ID3') || (bytes[0] === 0xff && (bytes[1]! & 0xe0) === 0xe0) || ascii(bytes, 4, 'ftyp') || ascii(bytes, 0, 'RIFF') ||
        ascii(bytes, 0, 'FORM') || ascii(bytes, 0, 'fLaC') || ascii(bytes, 0, 'OggS')
      );
    case 'text':
    case 'code':
      // Text has no NUL bytes; a binary plist or a misnamed binary does.
      return !bytes.subarray(0, 8192).includes(0);
    case 'none':
      return false;
  }
}

export type Preview =
  | { state: 'loading' }
  | { state: 'image' | 'pdf'; url: string }
  | { state: 'video' | 'audio'; url: string }
  | { state: 'text'; text: string; truncated: boolean; code: boolean }
  /** Folders, other kinds, and files whose bytes can't be shown. */
  | { state: 'info' }
  | { state: 'large' }
  | { state: 'failed'; message: string }
  | { state: 'offline' };

const demo = () => !!document.documentElement.dataset.demo;

async function failure(response: Response): Promise<ApiError> {
  const body = (await response.json().catch(() => ({}))) as { error?: { message?: string; code?: string } };
  return new ApiError(body.error?.code ?? 'internal', body.error?.message ?? 'Couldn’t load the preview.', response.status);
}

/** Read at most `limit` bytes and say whether the file went on past them. */
async function readHead(response: Response, limit: number): Promise<{ bytes: Uint8Array; more: boolean }> {
  const reader = response.body?.getReader();
  if (!reader) {
    const all = new Uint8Array(await response.arrayBuffer());
    return { bytes: all.subarray(0, limit), more: all.length > limit };
  }
  const chunks: Uint8Array[] = [];
  let length = 0;
  let more = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    length += value.length;
    if (length > limit) {
      more = true;
      void reader.cancel().catch(() => undefined);
      break;
    }
  }
  const bytes = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  return { bytes: bytes.subarray(0, limit), more };
}

async function load(instanceId: string, item: FileItem, kind: PreviewKind, signal: AbortSignal): Promise<{ preview: Preview; revoke?: string }> {
  const url = api.downloadUrl(instanceId, item.path);
  const limit = BLOB_LIMIT[kind];
  if (limit && item.size > limit) return { preview: { state: 'large' } };

  // Video and audio stream from the server; the demo answers only through the transport.
  if ((kind === 'video' || kind === 'audio') && !demo()) return { preview: { state: kind, url } };

  const response = await transport.fetch(url, { method: 'GET', credentials: 'same-origin', cache: 'no-store', signal });
  if (!response.ok) throw await failure(response);

  if (kind === 'text' || kind === 'code') {
    const { bytes, more } = await readHead(response, TEXT_LIMIT);
    if (!looksLike(kind, bytes)) return { preview: { state: 'info' } };
    // A cut can land inside a character; the decoder drops the broken tail.
    const text = new TextDecoder('utf-8').decode(bytes, { stream: more });
    return { preview: { state: 'text', text, truncated: more || item.size > TEXT_LIMIT, code: kind === 'code' } };
  }

  const blob = await response.blob();
  if (!looksLike(kind, new Uint8Array(await blob.slice(0, 1024).arrayBuffer()))) return { preview: { state: 'info' } };
  const typed = new Blob([blob], { type: MIME[extension(item.name)] ?? blob.type });
  const objectUrl = URL.createObjectURL(typed);
  return { preview: { state: kind as 'image' | 'pdf' | 'video' | 'audio', url: objectUrl }, revoke: objectUrl };
}

/** The preview for `item`, reloaded when it changes; null item means closed. */
export function usePreview(instanceId: string, item: FileItem | null, running: boolean): [Preview, () => void] {
  const [result, setResult] = useState<{ path: string; preview: Preview } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const kind = item ? previewKind(item) : 'none';
  const path = item?.path ?? null;

  useEffect(() => {
    if (!item || !running || kind === 'none') return;
    const controller = new AbortController();
    let revoke: string | undefined;
    setResult(null);
    load(instanceId, item, kind, controller.signal).then(
      (done) => {
        revoke = done.revoke;
        if (controller.signal.aborted) {
          if (revoke) URL.revokeObjectURL(revoke);
          return;
        }
        setResult({ path: item.path, preview: done.preview });
      },
      (cause: unknown) => {
        if (controller.signal.aborted) return;
        const error = cause instanceof ApiError ? cause : null;
        const offline = error?.status === 503 || error?.code === 'unavailable';
        setResult({ path: item.path, preview: offline ? { state: 'offline' } : { state: 'failed', message: error?.message ?? 'Check the connection and try again.' } });
      },
    );
    return () => {
      controller.abort();
      if (revoke) URL.revokeObjectURL(revoke);
    };
    // The item's identity changes on every listing refresh; its path, size and date are what matter.
  }, [instanceId, path, item?.size, item?.modified, kind, running, attempt]);

  const retry = () => setAttempt((n) => n + 1);
  if (!item) return [{ state: 'info' }, retry];
  if (!running) return [{ state: 'offline' }, retry];
  if (kind === 'none') return [{ state: 'info' }, retry];
  if (!result || result.path !== item.path) return [{ state: 'loading' }, retry];
  return [result.preview, retry];
}

/** A media element that couldn't decode the file falls back to the info card. */
export function useMediaFallback(key: string | null) {
  const [broken, setBroken] = useState<string | null>(null);
  return { broken: broken !== null && broken === key, fail: () => setBroken(key) };
}
