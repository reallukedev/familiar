import { useState } from 'react';
import { ApiError, api } from '../../lib/api';
import { keys, updateData } from '../../lib/cache';
import type { AccessToken, Connections } from '../../lib/types';

export type CreatedToken = { token: AccessToken; secret: string };

/**
 * Creates a personal access token and adds it to the cached list. The secret
 * lives only in the caller's state: Familiar never returns it again.
 */
export function useCreateToken(instanceId: string) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(name: string, expiresInDays: number | null): Promise<CreatedToken | null> {
    setBusy(true);
    setError(null);
    try {
      const result = await api.createToken(instanceId, { name, ...(expiresInDays === null ? {} : { expires_in_days: expiresInDays }) });
      updateData<Connections>(keys.connections(instanceId), (data) => ({ ...data, tokens: [result.token, ...data.tokens.filter((token) => token.id !== result.token.id)] }));
      return result;
    } catch (cause) {
      // A cancelled password check is silent: the person chose not to go on.
      if (!(cause instanceof ApiError && cause.silent)) setError(cause instanceof ApiError ? cause.message : 'Couldn’t create the token. Check your connection and try again.');
      return null;
    } finally {
      setBusy(false);
    }
  }

  return { create, busy, error, clearError: () => setError(null) };
}
