import { useEffect, useRef, useState } from 'react';
import { notify } from '../lib/toast';
import { Icon } from './Icon';
import styles from './Copy.module.css';

async function writeClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Copy, then "Copied" with a check for two seconds, then back to Copy. */
export function useCopy(text: string) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    if (await writeClipboard(text)) {
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 2000);
    } else {
      notify.error(null, 'Couldn’t copy. Select the text and copy it yourself.');
    }
  };
  return { copied, copy };
}

export function CopyButton({ text, label = 'Copy', what }: { text: string; label?: string; what: string }) {
  const { copied, copy } = useCopy(text);
  return (
    <button type="button" className={styles.button} onClick={copy} aria-label={copied ? `${what} copied` : `${label} ${what}`} data-copied={copied || undefined}>
      <span className={styles.glyph} aria-hidden="true">
        <Icon name={copied ? 'check' : 'copy'} size={15} weight={2.1} />
      </span>
      <span className={styles.text} aria-hidden="true">
        <span data-visible={!copied || undefined}>{label}</span>
        <span data-visible={copied || undefined}>Copied</span>
      </span>
      <span className="visually-hidden" aria-live="polite">
        {copied ? 'Copied' : ''}
      </span>
    </button>
  );
}

/** A value to copy, such as a URL or a command, set in mono and selectable as one unit. */
export function CopyField({ value, what, prefix }: { value: string; what: string; prefix?: string }) {
  return (
    <div className={styles.field}>
      {prefix ? (
        <span className={styles.prefix} aria-hidden="true">
          {prefix}
        </span>
      ) : null}
      <code className={styles.value} translate="no">{value}</code>
      <CopyButton text={value} what={what} />
    </div>
  );
}
