import { createContext } from 'react';

export type ShellState = {
  kind: 'phone' | 'desktop';
  sidebarOpen: boolean;
  toggleSidebar: () => void;
  /** Desktop: the sidebar is drawn over the content (narrow windows). */
  sidebarOverlay: boolean;
};

export const ShellContext = createContext<ShellState>({ kind: 'phone', sidebarOpen: false, toggleSidebar: () => {}, sidebarOverlay: false });
