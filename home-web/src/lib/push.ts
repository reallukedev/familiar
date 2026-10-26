// Web Push for approval notifications: they arrive even with no Familiar tab open.
// The service worker (public/sw.js) shows them; this file registers it and keeps this
// browser's subscription in step with the server.
import { api } from './api';

const SW_URL = '/sw.js';

/** Service workers, the Push API and notifications all exist here. On iPhone and iPad
 * that means Familiar was added to the Home Screen and opened from there. */
export function pushSupported(): boolean {
  return (
    typeof navigator !== 'undefined' &&
    'serviceWorker' in navigator &&
    typeof window !== 'undefined' &&
    'PushManager' in window &&
    typeof Notification !== 'undefined'
  );
}

/** Register the worker at the site root. Safe to call repeatedly. */
export async function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return null;
  try {
    // `updateViaCache: 'none'`: a new sw.js is picked up on the next visit, never from cache.
    return await navigator.serviceWorker.register(SW_URL, { scope: '/', updateViaCache: 'none' });
  } catch {
    return null;
  }
}

async function registration(): Promise<ServiceWorkerRegistration | null> {
  if (!pushSupported()) return null;
  return (await navigator.serviceWorker.getRegistration('/')) ?? (await registerServiceWorker());
}

/** This browser's current subscription, if it has one. */
export async function currentSubscription(): Promise<PushSubscription | null> {
  try {
    const reg = await registration();
    return (await reg?.pushManager.getSubscription()) ?? null;
  } catch {
    return null;
  }
}

/** base64url (no padding) to bytes, for `applicationServerKey`. */
export function base64UrlToBytes(text: string): Uint8Array<ArrayBuffer> {
  const base64 = text.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(text.length / 4) * 4, '=');
  const binary = atob(base64);
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function sameKey(a: ArrayBuffer | null | undefined, b: Uint8Array): boolean {
  if (!a) return false;
  const left = new Uint8Array(a);
  return left.length === b.length && left.every((byte, i) => byte === b[i]);
}

/**
 * Subscribe this browser with the server's VAPID key and send the subscription to the
 * server. Permission must already be granted. An existing subscription made with
 * another key (a reset server, say) is replaced.
 */
export async function subscribePush(): Promise<void> {
  const reg = await registration();
  if (!reg) throw new Error('This browser can’t receive notifications.');
  await navigator.serviceWorker.ready;
  const { public_key } = await api.pushKey();
  const key = base64UrlToBytes(public_key);
  let subscription = await reg.pushManager.getSubscription();
  if (subscription && !sameKey(subscription.options.applicationServerKey, key)) {
    await subscription.unsubscribe().catch(() => false);
    subscription = null;
  }
  subscription ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  const json = subscription.toJSON();
  if (!json.endpoint || !json.keys?.p256dh || !json.keys.auth) throw new Error('The browser gave an incomplete subscription.');
  await api.addPushSubscription({ endpoint: json.endpoint, keys: { p256dh: json.keys.p256dh, auth: json.keys.auth } });
}

/** Unsubscribe this browser and tell the server to forget it. */
export async function unsubscribePush(): Promise<void> {
  const subscription = await currentSubscription();
  if (!subscription) return;
  const { endpoint } = subscription;
  await subscription.unsubscribe().catch(() => false);
  await api.removePushSubscription(endpoint);
}
