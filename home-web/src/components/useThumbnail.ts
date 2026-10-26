import { useEffect, useRef, useState } from 'react';
import { api } from '../lib/api';
import { transport } from '../lib/transport';

/**
 * The Mac's latest screen frame, refreshed every `everyMs` while the element is
 * on screen and the tab is visible. Frames are decoded before they swap in, so
 * the picture never flashes. Returns null when the Mac has no screen (204).
 */
export function useThumbnail(instanceId: string, enabled: boolean, everyMs = 5000) {
  const ref = useRef<HTMLElement | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    const node = ref.current;
    if (!node || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => setVisible(!!entry?.isIntersecting), { rootMargin: '120px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!enabled || !visible) return;
    let cancelled = false;
    const controller = new AbortController();
    const tick = async () => {
      if (document.visibilityState !== 'visible') return;
      try {
        const response = await transport.fetch(`${api.thumbnailUrl(instanceId)}?t=${Date.now()}`, { credentials: 'same-origin', cache: 'no-store', signal: controller.signal });
        if (cancelled) return;
        if (response.status === 204 || !response.ok) {
          setUrl(null);
          return;
        }
        const next = URL.createObjectURL(await response.blob());
        const image = new Image();
        image.src = next;
        await image.decode().catch(() => {});
        if (cancelled) {
          URL.revokeObjectURL(next);
          return;
        }
        setUrl(next);
      } catch {
        // Offline or aborted: keep the last frame.
      }
    };
    void tick();
    const timer = setInterval(() => void tick(), everyMs);
    const onVisible = () => document.visibilityState === 'visible' && void tick();
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      controller.abort();
      clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [instanceId, enabled, visible, everyMs]);

  useEffect(() => {
    if (!enabled) setUrl(null);
  }, [enabled]);

  // Each frame's object URL lives until the next frame replaces it or the hook unmounts.
  useEffect(() => (url ? () => URL.revokeObjectURL(url) : undefined), [url]);

  return { ref, url };
}
