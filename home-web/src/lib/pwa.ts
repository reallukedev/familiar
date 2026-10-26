// Loaded from index.html beside main.tsx. Registers the service worker on every start,
// which makes Familiar installable (Add to Dock, Add to Home Screen, Install), and
// routes notification clicks the worker forwards to an open tab.
import { registerServiceWorker } from './push';
import { navigate } from './router';

if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator) {
  navigator.serviceWorker.addEventListener('message', (event: MessageEvent) => {
    const data = event.data as { type?: unknown; url?: unknown } | null;
    if (data?.type !== 'dot-home:navigate' || typeof data.url !== 'string' || !data.url.startsWith('/') || data.url.startsWith('//')) return;
    if (data.url !== location.pathname + location.search) navigate(data.url);
  });
  navigator.serviceWorker.startMessages();
  // After the page has loaded, so registering never competes with the app for the network.
  if (document.readyState === 'complete') void registerServiceWorker();
  else addEventListener('load', () => void registerServiceWorker(), { once: true });
}
