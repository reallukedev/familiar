import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type ApiError } from '../../lib/api';
import type { FolderListing } from '../../lib/types';

// Finder's Calculate Size, done from the browser: there's no folder-size
// endpoint, so the folder is walked one listing at a time.

export const WALK_CONCURRENCY = 4;
export const WALK_CAP = 50_000;

export type SizeTotal = { bytes: number; items: number; /** The cap was hit, a listing was cut off or a folder couldn't be read. */ atLeast: boolean };

export type FolderSize =
  | { state: 'idle' }
  | ({ state: 'running' } & SizeTotal)
  | ({ state: 'done' } & SizeTotal)
  | { state: 'failed'; message: string };

type List = (path: string) => Promise<FolderListing>;

/**
 * Total a folder and everything in it. Up to `concurrency` listings load at
 * once; aliases and symlinks count as themselves and are never followed.
 * Stops after `cap` items. `onProgress` gets the running total.
 */
export async function walkFolder(
  root: string,
  list: List,
  { signal, onProgress, concurrency = WALK_CONCURRENCY, cap = WALK_CAP }: { signal?: AbortSignal; onProgress?: (total: SizeTotal) => void; concurrency?: number; cap?: number } = {},
): Promise<SizeTotal> {
  const total: SizeTotal = { bytes: 0, items: 0, atLeast: false };
  const queue: string[] = [root];
  const seen = new Set<string>([root]);
  let active = 0;
  let firstError: unknown = null;
  let rootRead = false;

  return new Promise<SizeTotal>((resolve, reject) => {
    const finish = () => {
      if (signal?.aborted) reject(new DOMException('Cancelled', 'AbortError'));
      else if (!rootRead && firstError) reject(firstError);
      else resolve({ ...total });
    };
    const pump = () => {
      if (signal?.aborted) return finish();
      while (active < concurrency && queue.length && total.items < cap) {
        const path = queue.shift()!;
        active += 1;
        list(path).then(
          (listing) => {
            if (signal?.aborted) return;
            if (path === root) rootRead = true;
            if (listing.truncated) total.atLeast = true;
            for (const entry of listing.entries) {
              if (total.items >= cap) {
                total.atLeast = true;
                break;
              }
              total.items += 1;
              total.bytes += entry.size || 0;
              // Only real folders (packages included) are walked; never symlinks or aliases.
              if (entry.kind === 'dir' && !seen.has(entry.path)) {
                seen.add(entry.path);
                queue.push(entry.path);
              }
            }
            onProgress?.({ ...total });
          },
          (cause: unknown) => {
            // A folder macOS won't open (privacy, gone): count what can be read.
            if (path === root) firstError = cause;
            total.atLeast = true;
          },
        ).finally(() => {
          active -= 1;
          if (total.items >= cap && queue.length) total.atLeast = true;
          if (active === 0 && (!queue.length || total.items >= cap || signal?.aborted)) finish();
          else pump();
        });
      }
      if (active === 0) finish();
    };
    signal?.addEventListener('abort', () => active === 0 || finish(), { once: true });
    pump();
  });
}

/**
 * Calculate Size for the folder at `path`. Results stay cached in `cache`
 * (the caller clears it when Quick Look closes); a walk in progress stops
 * when the folder changes.
 */
export function useFolderSize(instanceId: string, path: string | null, cache: Map<string, SizeTotal>) {
  const [run, setRun] = useState<{ path: string; size: FolderSize } | null>(null);
  const controller = useRef<AbortController | null>(null);

  const cancel = useCallback(() => {
    controller.current?.abort();
    controller.current = null;
    setRun(null);
  }, []);

  // Moving to another item stops the walk.
  useEffect(() => cancel, [path, cancel]);

  const start = useCallback(() => {
    if (!path) return;
    controller.current?.abort();
    const abort = new AbortController();
    controller.current = abort;
    let last = 0;
    setRun({ path, size: { state: 'running', bytes: 0, items: 0, atLeast: false } });
    walkFolder(path, (at) => api.files(instanceId, at), {
      signal: abort.signal,
      onProgress: (total) => {
        // A few repaints a second is enough for a running total.
        const now = performance.now();
        if (now - last < 120) return;
        last = now;
        setRun({ path, size: { state: 'running', ...total } });
      },
    }).then(
      (total) => {
        if (abort.signal.aborted) return;
        cache.set(path, total);
        controller.current = null;
        setRun({ path, size: { state: 'done', ...total } });
      },
      (cause: unknown) => {
        if (abort.signal.aborted) return;
        controller.current = null;
        setRun({ path, size: { state: 'failed', message: (cause as ApiError)?.message ?? 'Couldn’t calculate the size.' } });
      },
    );
  }, [instanceId, path, cache]);

  const cached = path ? cache.get(path) : undefined;
  const size: FolderSize = cached ? { state: 'done', ...cached } : run && run.path === path ? run.size : { state: 'idle' };
  return { size, start, cancel };
}
