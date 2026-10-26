// The signed-in person's preferences, saved on the server so they follow them
// between devices. Reads fall back to the defaults until the server answers,
// and changes apply at once, then roll back if the server refuses them.
import { api } from './api';
import { keys, peek, setData, useQuery } from './cache';
import { notify } from './toast';
import { DEFAULT_PREFERENCES, type Preferences } from './types';

export function usePreferences(): Preferences {
  const query = useQuery<Preferences>(keys.preferences, api.preferences, { staleMs: 60_000 });
  return { ...DEFAULT_PREFERENCES, ...query.data };
}

/** Read outside React (event handlers, the screen's key hooks). */
export function currentPreferences(): Preferences {
  return { ...DEFAULT_PREFERENCES, ...peek<Preferences>(keys.preferences) };
}

export async function setPreferences(patch: Partial<Preferences>): Promise<boolean> {
  const before = currentPreferences();
  setData<Preferences>(keys.preferences, { ...before, ...patch });
  try {
    setData<Preferences>(keys.preferences, await api.setPreferences(patch));
    return true;
  } catch (error) {
    setData<Preferences>(keys.preferences, before);
    notify.error(error);
    return false;
  }
}
