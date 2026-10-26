import { api, hooks } from './api';
import { clearCache, fetchKey, invalidate, keys, peek, setData, useQuery } from './cache';
import { navigate } from './router';
import { PUBLIC_ROUTES, parseRoute } from './routes';
import { createStore } from './store';
import type { SessionInfo, User } from './types';

/** Set when the server says this account must enroll in two-factor first. */
export const totpGate = createStore(false);

/** Enrollment finished: drop the gate and pick up where the person was going. */
export async function finishEnrollment(user: User) {
  const session = peek<SessionInfo>(keys.session);
  if (session) setData(keys.session, { ...session, user });
  totpGate.set(false);
  await refreshSession().catch(() => {});
  invalidate();
}

export function useSession() {
  return useQuery<SessionInfo>(keys.session, api.session, { staleMs: 60_000 });
}

export function refreshSession() {
  return fetchKey(keys.session, api.session);
}

/** After sign-in or setup the server answers with the new session. */
export function adoptSession(session: SessionInfo) {
  clearCache();
  setData(keys.session, session);
}

export async function signOut() {
  try {
    await api.logout();
  } finally {
    const session = peek<SessionInfo>(keys.session);
    clearCache();
    if (session) setData(keys.session, { ...session, user: null, csrf: null });
    navigate('/signin', { replace: true });
  }
}

export function installSessionHooks() {
  hooks.csrf = () => peek<SessionInfo>(keys.session)?.csrf ?? null;
  hooks.refreshSession = async () => {
    await refreshSession().catch(() => {});
  };
  hooks.totpSetupRequired = () => totpGate.set(true);
  hooks.unauthenticated = () => {
    const session = peek<SessionInfo>(keys.session);
    if (session?.user) setData(keys.session, { ...session, user: null, csrf: null });
    const route = parseRoute(location.pathname);
    if (PUBLIC_ROUTES.has(route.name)) return;
    const next = location.pathname + location.search;
    navigate(`/signin?next=${encodeURIComponent(next)}`, { replace: true });
  };
}
