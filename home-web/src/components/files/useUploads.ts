import { useCallback, useSyncExternalStore } from 'react';
import { ApiError, api } from '../../lib/api';
import { keys, updateData } from '../../lib/cache';
import type { FileItem, FolderListing } from '../../lib/types';

export type Upload = {
  key: string;
  file: File;
  /** The folder as the page keeps it in the URL (`~/Documents`). */
  folder: string;
  folderName: string;
  status: 'waiting' | 'uploading' | 'done' | 'failed';
  /** 0 to 1 when the transport reports it; null shows an indeterminate bar. */
  progress: number | null;
  error?: string;
  /** The name the Mac gave it when the chosen one was taken ("report 2.pdf"). */
  savedAs?: string;
};

export const LIMIT = 2_000_000_000;
const AT_ONCE = 2;
const CLEAR_DONE_MS = 4000;
const TOO_BIG = 'Files up to 2 GB can be uploaded here. For bigger files, use AirDrop or a shared folder on the Mac.';

let counter = 0;

/** Put a new or renamed item into a cached listing so it shows at once. */
export function placeInListing(instanceId: string, folder: string, item: FileItem) {
  updateData<FolderListing>(keys.files(instanceId, folder), (listing) => ({
    ...listing,
    entries: [...listing.entries.filter((entry) => entry.name !== item.name), item],
  }));
}

// The queue lives outside React so leaving the Files page (or switching Macs)
// doesn't drop it: uploads keep going and the list is there on return.
const queues = new Map<string, Upload[]>();
const listeners = new Map<string, Set<() => void>>();
const controllers = new Map<string, AbortController>();
const NONE: Upload[] = [];

function list(instanceId: string): Upload[] {
  return queues.get(instanceId) ?? NONE;
}

function setList(instanceId: string, fn: (list: Upload[]) => Upload[]) {
  queues.set(instanceId, fn(list(instanceId)));
  listeners.get(instanceId)?.forEach((listener) => listener());
  guardUnload();
  pump(instanceId);
}

function patch(instanceId: string, key: string, next: Partial<Upload>) {
  setList(instanceId, (items) => items.map((item) => (item.key === key ? { ...item, ...next } : item)));
}

const active = (item: Upload) => item.status === 'waiting' || item.status === 'uploading';

/** Closing or reloading the tab would stop uploads; let the browser ask first. */
function onBeforeUnload(event: BeforeUnloadEvent) {
  event.preventDefault();
}
let guarding = false;
function guardUnload() {
  const busy = [...queues.values()].some((items) => items.some(active));
  if (busy === guarding || typeof window === 'undefined') return;
  guarding = busy;
  if (busy) window.addEventListener('beforeunload', onBeforeUnload);
  else window.removeEventListener('beforeunload', onBeforeUnload);
}

/** Start waiting uploads, two at a time per Mac. */
function pump(instanceId: string) {
  const items = list(instanceId);
  const running = items.filter((item) => item.status === 'uploading').length;
  const next = items.filter((item) => item.status === 'waiting').slice(0, Math.max(0, AT_ONCE - running));
  for (const item of next) start(instanceId, item);
}

function start(instanceId: string, item: Upload) {
  // A nested pump (every patch pumps) may have started it already.
  if (list(instanceId).find((entry) => entry.key === item.key)?.status !== 'waiting') return;
  const controller = new AbortController();
  controllers.set(item.key, controller);
  patch(instanceId, item.key, { status: 'uploading', progress: null, error: undefined });
  // Progress events come many times a second; re-render only when the percent moves.
  let shown = -1;
  const onProgress = (fraction: number) => {
    const percent = Math.floor(Math.min(1, Math.max(0, fraction)) * 100);
    if (percent === shown || controller.signal.aborted) return;
    shown = percent;
    patch(instanceId, item.key, { progress: percent / 100 });
  };
  api
    .uploadFile(instanceId, item.folder, item.file, { signal: controller.signal, onProgress })
    .then((saved) => {
      placeInListing(instanceId, item.folder, saved);
      patch(instanceId, item.key, { status: 'done', progress: 1, savedAs: saved.name !== item.file.name ? saved.name : undefined });
      setTimeout(() => setList(instanceId, (items) => items.filter((entry) => entry.key !== item.key || entry.status !== 'done')), CLEAR_DONE_MS);
    })
    .catch((cause: unknown) => {
      // Cancelled from the list: it's already gone. Stopped some other way (signing out): say so.
      const error = cause instanceof ApiError ? cause.message : cause instanceof DOMException && cause.name === 'AbortError' ? 'The upload was stopped.' : 'The upload didn’t finish. Try again.';
      patch(instanceId, item.key, { status: 'failed', error });
    })
    .finally(() => controllers.delete(item.key));
}

/** A queue of uploads to one Mac, two at a time, that outlives folder changes and leaving the page. */
export function useUploads(instanceId: string) {
  const subscribe = useCallback((listener: () => void) => {
    let set = listeners.get(instanceId);
    if (!set) listeners.set(instanceId, (set = new Set()));
    set.add(listener);
    return () => {
      set.delete(listener);
    };
  }, [instanceId]);
  const snapshot = useCallback(() => list(instanceId), [instanceId]);
  const uploads = useSyncExternalStore(subscribe, snapshot, snapshot);

  const add = useCallback((files: File[], folder: string, folderName: string) => {
    const fresh: Upload[] = files.map((file) => {
      counter += 1;
      const tooBig = file.size > LIMIT;
      return { key: `u${counter}`, file, folder, folderName, status: tooBig ? 'failed' : 'waiting', progress: null, error: tooBig ? TOO_BIG : undefined };
    });
    setList(instanceId, (items) => [...items, ...fresh]);
  }, [instanceId]);

  const cancel = useCallback((key: string) => {
    // Drop it first so the abort lands on an upload that's no longer listed.
    setList(instanceId, (items) => items.filter((item) => item.key !== key));
    controllers.get(key)?.abort();
  }, [instanceId]);

  const retry = useCallback((key: string) => patch(instanceId, key, { status: 'waiting', error: undefined, progress: null }), [instanceId]);

  const clearFinished = useCallback(() => setList(instanceId, (items) => items.filter(active)), [instanceId]);

  return { uploads, add, cancel, retry, clearFinished };
}
