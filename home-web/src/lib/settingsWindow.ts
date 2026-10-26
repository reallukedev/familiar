// Opens and closes the Settings window from anywhere (sidebar, ⌘, , the
// command palette, the phone's Account tab).
import { useSyncExternalStore } from 'react';

export type SettingsTab = 'general' | 'screen' | 'notifications' | 'shortcuts';

let state: { open: boolean; tab: SettingsTab } = { open: false, tab: 'general' };
const listeners = new Set<() => void>();

function set(next: typeof state) {
  state = next;
  listeners.forEach((listener) => listener());
}

export function openSettings(tab?: SettingsTab) {
  set({ open: true, tab: tab ?? state.tab });
}
export function closeSettings() {
  set({ ...state, open: false });
}
export function selectSettingsTab(tab: SettingsTab) {
  set({ ...state, tab });
}

export function useSettingsWindow() {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
}
