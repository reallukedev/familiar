import { use, useEffect, useRef, useState, type ReactNode } from 'react';
import { Icon } from '../components/Icon';
import { IconButton } from '../components/Button';
import { Tip } from '../components/Tip';
import { goBack, isPlainClick } from '../lib/router';
import { isMacKeyboard } from '../lib/platform';
import { ShellContext } from './context';
import styles from './Page.module.css';

type Props = {
  title: string;
  /** Where back goes on a phone, and its label (the previous screen's title). */
  back?: { href: string; label: string };
  /** Toolbar items: right side of the nav bar or Mac toolbar. */
  actions?: ReactNode;
  /** regular: settings column; wide: overview with the display; full: edge to edge (the live screen). */
  width?: 'regular' | 'wide' | 'full';
  children: ReactNode;
};

/** A screen. Draws an iOS navigation bar on a phone and a macOS toolbar on a Mac. */
export function Page(props: Props) {
  const shell = use(ShellContext);
  useEffect(() => {
    document.title = props.title === 'Familiar' ? 'Familiar' : `${props.title} · Familiar`;
  }, [props.title]);
  return shell.kind === 'phone' ? <PhonePage {...props} /> : <MacPage {...props} />;
}

function PhonePage({ title, back, actions, width = 'regular', children }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  const [collapsed, setCollapsed] = useState(false);
  const [scrolled, setScrolled] = useState(false);
  const large = width !== 'full';

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 2);
    onScroll();
    addEventListener('scroll', onScroll, { passive: true });
    return () => removeEventListener('scroll', onScroll);
  }, []);

  useEffect(() => {
    const node = heading.current;
    if (!node) return;
    const observer = new IntersectionObserver(([entry]) => setCollapsed(!entry?.isIntersecting), { rootMargin: '-52px 0px 0px 0px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <div className={`${styles.phone} ${width === 'full' ? styles.phoneFull : ''}`}>
      <header className={`${styles.navbar} ${scrolled || !large ? styles.material : ''}`}>
        <div className={styles.navInner}>
          <div className={styles.navLead}>
            {back ? (
              <a
                className={styles.back}
                href={back.href}
                onClick={(event) => {
                  if (!isPlainClick(event)) return;
                  event.preventDefault();
                  goBack(back.href);
                }}
              >
                <Icon name="chevron-left" size={24} weight={2.3} />
                <span className={styles.backLabel}>{back.label}</span>
              </a>
            ) : null}
          </div>
          <div className={`t-headline ${styles.inlineTitle} ${!large || collapsed ? styles.shown : ''}`} aria-hidden={large && !collapsed}>
            {title}
          </div>
          <div className={styles.navTrail}>{actions}</div>
        </div>
      </header>
      {large ? (
        <h1 className={`t-large ${styles.large}`} ref={heading}>
          {title}
        </h1>
      ) : (
        <h1 className="visually-hidden">{title}</h1>
      )}
      <div className={styles.phoneContent}>{children}</div>
    </div>
  );
}

function MacPage({ title, actions, width = 'regular', children }: Props) {
  const shell = use(ShellContext);
  const [scrolled, setScrolled] = useState(false);
  const sentinel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const node = sentinel.current;
    if (!node) return;
    const observer = new IntersectionObserver(([entry]) => setScrolled(!entry?.isIntersecting));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <div className={`${styles.mac} ${styles[`mac-${width}`]}`}>
      <header className={`${styles.toolbar} ${scrolled ? styles.toolbarScrolled : ''}`}>
        <div className={styles.toolbarInner}>
        {shell.sidebarOverlay || !shell.sidebarOpen ? (
          <Tip label={shell.sidebarOpen ? 'Hide Sidebar' : 'Show Sidebar'} shortcut={isMacKeyboard ? '⌃⌘S' : 'Ctrl+Alt+S'}>
            <IconButton label={shell.sidebarOpen ? 'Hide sidebar' : 'Show sidebar'} onClick={shell.toggleSidebar} aria-expanded={shell.sidebarOpen}>
              <Icon name="sidebar" size={18} weight={1.7} />
            </IconButton>
          </Tip>
        ) : null}
        <h1 className={`${styles.toolbarTitle}`}>{title}</h1>
        <div className={styles.toolbarActions} data-toolbar>
          {actions}
        </div>
        </div>
      </header>
      <div ref={sentinel} className={styles.sentinel} aria-hidden="true" />
      <div className={styles.macContent}>{children}</div>
    </div>
  );
}
