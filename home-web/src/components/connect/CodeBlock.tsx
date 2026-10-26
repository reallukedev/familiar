import type { ReactNode } from 'react';
import { CopyButton } from '../Copy';
import styles from './CodeBlock.module.css';

type Props = {
  code: string;
  /** What the block is, for the header and the copy button's name: "Terminal", "~/.cursor/mcp.json". */
  label: string;
  what: string;
  /** A secret inside the code, marked so it's easy to spot before pasting it anywhere. */
  secret?: string | null;
  /** Shown instead of the copy button when the code isn't ready to use yet. */
  pending?: ReactNode;
};

/** A ready-to-paste command or config file, with a Copy button that copies all of it. */
export function CodeBlock({ code, label, what, secret, pending }: Props) {
  return (
    <figure className={styles.block}>
      <figcaption className={styles.head}>
        <span className={styles.label} translate="no">
          {label}
        </span>
        {pending ? <span className={`t-footnote secondary ${styles.pending}`}>{pending}</span> : <CopyButton text={code} what={what} />}
      </figcaption>
      <pre className={styles.pre} tabIndex={0} aria-label={`${label}: ${what}`}>
        <code translate="no">{secret ? mark(code, secret) : code}</code>
      </pre>
    </figure>
  );
}

function mark(code: string, secret: string): ReactNode {
  const parts = code.split(secret);
  return parts.flatMap((part, index) =>
    index === 0
      ? [part]
      : [
          <mark key={index} className={styles.secret}>
            {secret}
          </mark>,
          part,
        ],
  );
}
