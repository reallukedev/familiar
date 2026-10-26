// Familiar service worker: approval notifications through Web Push, and what makes
// the portal installable. It never caches anything and has no fetch handler, so every
// request goes straight to the network as if this worker weren't here.

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

/** Only paths on this origin; anything else opens the portal's home. */
function samePath(url) {
  try {
    const target = new URL(typeof url === 'string' ? url : '/', self.location.origin);
    return target.origin === self.location.origin ? target.pathname + target.search + target.hash : '/';
  } catch {
    return '/';
  }
}

self.addEventListener('push', (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    // Not JSON: still show something, since a push must always show a notification.
  }
  const title = typeof data.title === 'string' && data.title ? data.title : 'Familiar';
  const options = {
    body: typeof data.body === 'string' ? data.body : '',
    icon: '/icon-192.png',
    requireInteraction: true,
    data: { url: samePath(data.url), approval_id: data.approval_id ?? null, instance_id: data.instance_id ?? null },
  };
  if (typeof data.approval_id === 'string') {
    // One notification per request, even if it's pushed twice or a tab also shows it.
    options.tag = `approval:${data.approval_id}`;
    options.renotify = false;
  }
  event.waitUntil(self.registration.showNotification(title, options));
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const path = samePath(event.notification.data && event.notification.data.url);
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const ours = windows.filter((client) => {
        try {
          return new URL(client.url).origin === self.location.origin;
        } catch {
          return false;
        }
      });
      const target = ours.find((client) => client.focused) ?? ours.find((client) => client.visibilityState === 'visible') ?? ours[0];
      if (target) {
        const focused = await target.focus().catch(() => target);
        // The page routes on its own (src/lib/pwa.ts), keeping the open Mac screen alive.
        (focused ?? target).postMessage({ type: 'dot-home:navigate', url: path });
        return;
      }
      await self.clients.openWindow(path);
    })(),
  );
});
