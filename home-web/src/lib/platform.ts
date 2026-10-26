import { useSyncExternalStore } from 'react';

// Two shells, chosen by width: the iOS idiom below 768 px, the macOS idiom at
// 768 px and up (tablets get the Mac layout with a collapsible sidebar).
export const DESKTOP_QUERY = '(min-width: 768px)';
export const WIDE_QUERY = '(min-width: 1080px)';

function media(query: string) {
  const list = typeof matchMedia === 'undefined' ? null : matchMedia(query);
  return {
    subscribe: (listener: () => void) => {
      list?.addEventListener('change', listener);
      return () => list?.removeEventListener('change', listener);
    },
    get: () => list?.matches ?? false,
  };
}

const cache = new Map<string, ReturnType<typeof media>>();
export function useMedia(query: string): boolean {
  let entry = cache.get(query);
  if (!entry) cache.set(query, (entry = media(query)));
  return useSyncExternalStore(entry.subscribe, entry.get, () => false);
}

export type Platform = 'phone' | 'desktop';
export function usePlatform(): Platform {
  return useMedia(DESKTOP_QUERY) ? 'desktop' : 'phone';
}

/** The same, read once outside React. */
export function reducedMotion(): boolean {
  return typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function useReducedMotion(): boolean {
  return useMedia('(prefers-reduced-motion: reduce)');
}

export const isMacKeyboard = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
