import { useSyncExternalStore } from 'react';
import { flushSync } from 'react-dom';
import { parseRoute, type Route } from './routes';

export type Direction = 'forward' | 'back' | 'none';
type Location = { path: string; search: string; route: Route; direction: Direction };

let depth = 0;
let current: Location = read('none');
const listeners = new Set<() => void>();

/** The phone shell installs this to animate pushes and pops; the desktop leaves it null. */
let animator: ((direction: Direction, update: () => void) => void) | null = null;
export function setNavigationAnimator(fn: typeof animator) {
  animator = fn;
}

function read(direction: Direction): Location {
  if (typeof location === 'undefined') return { path: '/', search: '', route: { name: 'macs' }, direction };
  return { path: location.pathname, search: location.search, route: parseRoute(location.pathname), direction };
}

// Where each history entry was scrolled, so Back returns to the same spot.
const scrolls = new Map<number, number>();

function scroller(): { get: () => number; set: (top: number) => void } {
  const root = document.querySelector<HTMLElement>('[data-scroll-root]');
  return root
    ? { get: () => root.scrollTop, set: (top) => root.scrollTo({ top }) }
    : { get: () => window.scrollY, set: (top) => window.scrollTo({ top }) };
}

/**
 * Renders the new location, then scrolls it. The scroll happens inside the
 * update so the page being left keeps its position in the transition's
 * snapshot instead of jumping to the top first.
 */
function emit(direction: Direction, scrollTop: number | null = 0) {
  const apply = () => {
    current = read(direction);
    listeners.forEach((listener) => listener());
    if (scrollTop !== null) scroller().set(scrollTop);
  };
  if (animator && direction !== 'none') animator(direction, () => flushSync(apply));
  else apply();
}

if (typeof window !== 'undefined') {
  depth = (history.state?.depth as number | undefined) ?? 0;
  if (history.state?.depth === undefined) history.replaceState({ depth: 0 }, '');
  history.scrollRestoration = 'manual';
  addEventListener('popstate', (event) => {
    const next = (event.state?.depth as number | undefined) ?? 0;
    const direction: Direction = next < depth ? 'back' : 'forward';
    scrolls.set(depth, scroller().get());
    depth = next;
    emit(direction, scrolls.get(next) ?? 0);
  });
}

export function navigate(to: string, options: { replace?: boolean; direction?: Direction } = {}) {
  const url = new URL(to, location.origin);
  const target = url.pathname + url.search + url.hash;
  if (target === location.pathname + location.search + location.hash) return;
  if (options.replace) history.replaceState({ depth }, '', target);
  else {
    scrolls.set(depth, scroller().get());
    depth += 1;
    history.pushState({ depth }, '', target);
  }
  emit(options.direction ?? (options.replace ? 'none' : 'forward'), url.hash ? null : 0);
}

/** Pop when there is in-app history, otherwise replace with the parent. */
export function goBack(fallback: string) {
  if (depth > 0) history.back();
  else navigate(fallback, { replace: true, direction: 'back' });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useLocation(): Location {
  return useSyncExternalStore(subscribe, () => current, () => current);
}

export function useSearchParam(name: string): string | null {
  const { search } = useLocation();
  return new URLSearchParams(search).get(name);
}

/** A primary click with no modifiers: the kind an in-app link handles itself (others open a tab or window). */
export function isPlainClick(event: React.MouseEvent) {
  return !event.defaultPrevented && event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

/** Click handler for in-app anchors: keeps modified clicks (new tab) native. */
export function onLinkClick(event: React.MouseEvent<HTMLAnchorElement>, options?: { direction?: Direction; replace?: boolean }) {
  if (!isPlainClick(event)) return;
  const href = event.currentTarget.getAttribute('href');
  if (!href || !href.startsWith('/')) return;
  event.preventDefault();
  navigate(href, options);
}

/** Scroll the element the URL's #fragment names into view; a malformed fragment is ignored. */
export function scrollToHash(block: ScrollLogicalPosition) {
  if (!location.hash) return;
  let id: string;
  try {
    id = decodeURIComponent(location.hash.slice(1));
  } catch {
    return;
  }
  document.getElementById(id)?.scrollIntoView({ block });
}
