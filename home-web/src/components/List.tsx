import { useId, type ReactNode } from 'react';
import { onLinkClick } from '../lib/router';
import { Icon } from './Icon';
import styles from './List.module.css';

/** A grouped inset list: optional header, cells, optional footnote. */
export function Section({ title, accessory, footer, children, id, live }: { title?: ReactNode; accessory?: ReactNode; footer?: ReactNode; children: ReactNode; id?: string; live?: 'polite' }) {
  const generated = useId();
  const headingId = id ?? generated;
  return (
    <section className={styles.section} aria-labelledby={title ? headingId : undefined}>
      {title || accessory ? (
        <div className={styles.head}>
          {title ? (
            <h2 className={`t-title3 ${styles.title}`} id={headingId}>
              {title}
            </h2>
          ) : (
            <span />
          )}
          {accessory}
        </div>
      ) : null}
      <div className={styles.cells} aria-live={live}>
        {children}
      </div>
      {footer ? <div className={`t-footnote secondary ${styles.foot}`}>{footer}</div> : null}
    </section>
  );
}

type RowProps = {
  title: ReactNode;
  subtitle?: ReactNode;
  value?: ReactNode;
  leading?: ReactNode;
  trailing?: ReactNode;
  href?: string;
  onClick?: () => void;
  tone?: 'default' | 'tint' | 'destructive';
  chevron?: boolean;
  disabled?: boolean;
  selected?: boolean;
  highlight?: boolean;
  label?: string;
  /** Rows with several lines of text align their controls to the top. */
  align?: 'center' | 'top';
  className?: string;
};

export function Row({ title, subtitle, value, leading, trailing, href, onClick, tone = 'default', chevron = !!href, disabled, selected, highlight, label, align = 'center', className = '' }: RowProps) {
  const content = (
    <>
      {leading ? <span className={styles.leading}>{leading}</span> : null}
      <span className={styles.main}>
        <span className={styles.text}>
          <span className={`${styles.rowTitle} ${styles[`tone-${tone}`]}`}>{title}</span>
          {subtitle ? <span className={`${styles.subtitle} secondary`}>{subtitle}</span> : null}
        </span>
        {value !== undefined && value !== null ? <span className={`${styles.value} secondary tabular`}>{value}</span> : null}
        {trailing ? <span className={styles.trailing}>{trailing}</span> : null}
        {chevron ? (
          <span className={styles.chevron}>
            <Icon name="chevron-right" size={16} weight={2.4} />
          </span>
        ) : null}
      </span>
    </>
  );
  const classes = `${styles.row} ${align === 'top' ? styles.top : ''} ${highlight ? styles.highlight : ''} ${className}`;
  if (href && !disabled) {
    return (
      <a className={`${classes} ${styles.interactive}`} href={href} onClick={(event) => onLinkClick(event)} aria-label={label} aria-current={selected ? 'page' : undefined}>
        {content}
      </a>
    );
  }
  if (onClick) {
    return (
      <button type="button" className={`${classes} ${styles.interactive}`} onClick={onClick} disabled={disabled} aria-label={label}>
        {content}
      </button>
    );
  }
  return <div className={`${classes} ${disabled ? styles.disabled : ''}`}>{content}</div>;
}

/** A colored square glyph tile, as in iOS Settings. Color is category only. */
export function GlyphTile({ children, color = 'gray' }: { children: ReactNode; color?: 'blue' | 'green' | 'orange' | 'red' | 'gray' | 'indigo' }) {
  return <span className={`${styles.tile} ${styles[`tile-${color}`]}`}>{children}</span>;
}

/** Text inside a cell, for explanations that belong to the group. */
export function CellText({ children }: { children: ReactNode }) {
  return <div className={`${styles.cellText} t-subhead`}>{children}</div>;
}

export function CellBody({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`${styles.cellBody} ${className}`}>{children}</div>;
}
