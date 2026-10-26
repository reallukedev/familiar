import { useCallback, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from 'react';
import { useMedia, WIDE_QUERY } from '../lib/platform';
import { useLocation } from '../lib/router';
import { CommandPalette } from './CommandPalette';
import { ShellContext } from './context';
import { Sidebar } from './Sidebar';
import styles from './DesktopShell.module.css';

/**
 * The macOS idiom: a translucent sidebar of Macs and their sections beside a
 * content pane with its own toolbar, like System Settings. Narrow windows
 * (tablets) tuck the sidebar away and slide it over the content.
 */
export function DesktopShell({ children }: { children: ReactNode }) {
  const wide = useMedia(WIDE_QUERY);
  const [pinned, setPinned] = useState(true);
  const [overlayOpen, setOverlayOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const { path } = useLocation();

  const sidebarOpen = wide ? pinned : overlayOpen;
  const toggleSidebar = useCallback(() => (wide ? setPinned((value) => !value) : setOverlayOpen((value) => !value)), [wide]);

  // The macOS type ramp and control sizes apply to everything, including
  // sheets and menus that render in portals outside this tree.
  useLayoutEffect(() => {
    document.documentElement.classList.add('mac');
    return () => document.documentElement.classList.remove('mac');
  }, []);

  // Navigating from the overlay sidebar puts it away.
  useEffect(() => setOverlayOpen(false), [path, wide]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = !!target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setPaletteOpen(true);
      } else if (event.key === '/' && !typing && !mod) {
        event.preventDefault();
        setPaletteOpen(true);
      } else if (event.ctrlKey && (event.metaKey || event.altKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        toggleSidebar();
      } else if (event.key === 'Escape' && !wide && overlayOpen) {
        setOverlayOpen(false);
      }
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [toggleSidebar, wide, overlayOpen]);

  const shell = useMemo(() => ({ kind: 'desktop' as const, sidebarOpen, toggleSidebar, sidebarOverlay: !wide }), [sidebarOpen, toggleSidebar, wide]);

  return (
    <ShellContext value={shell}>
      <div className={`${styles.window} ${sidebarOpen && wide ? styles.withSidebar : ''}`}>
        <a className={styles.skip} href="#content">
          Skip to content
        </a>
        <div className={`${styles.sidebarSlot} ${sidebarOpen ? styles.sidebarShown : ''} ${wide ? '' : styles.overlay}`} inert={!sidebarOpen}>
          <Sidebar onSearch={() => setPaletteOpen(true)} />
        </div>
        {!wide && overlayOpen ? <button type="button" className={styles.scrim} aria-label="Hide sidebar" onClick={() => setOverlayOpen(false)} /> : null}
        <main id="content" className={styles.content} data-scroll-root tabIndex={-1}>
          {children}
        </main>
      </div>
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
    </ShellContext>
  );
}
