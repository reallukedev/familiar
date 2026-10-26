import { memo } from 'react';
import { duration, stamp, when } from '../lib/format';
import type { Activity } from '../lib/types';
import { Icon } from './Icon';
import styles from './ActivityRow.module.css';

const OUTCOME = {
  ok: { icon: 'check', label: 'Done', className: styles.ok },
  error: { icon: 'exclamation', label: 'Failed', className: styles.error },
  denied: { icon: 'blocked', label: 'Denied', className: styles.denied },
  pending: { icon: 'clock', label: 'Waiting for approval', className: styles.pending },
} as const;

/** Why a request failed, in words (the server's codes, docs/home/API.md). */
const FAILURE: Record<string, string> = {
  TIMEOUT: 'Timed out',
  PERMISSION_DENIED: 'Blocked by macOS',
  APPROVAL_DENIED: 'You denied it',
  APPROVAL_PENDING: 'No answer in time',
  DENIED: 'Not allowed by Permissions',
  PAUSED: 'The assistant was paused',
  MAC_STARTING: 'The Mac was still starting',
  MAC_UNAVAILABLE: 'The Mac wasn’t running',
  STORAGE_LIMIT: 'Over your storage limit',
  NOT_FOUND: 'Couldn’t find what it asked for',
  INVALID_ARGS: 'The assistant sent an invalid request',
  UNKNOWN_TOOL: 'Tool not available on this Mac',
  OUTCOME_UNKNOWN: 'Lost touch with the Mac',
};

export function failureText(code: string | null): string {
  return (code && FAILURE[code]) ?? 'Something went wrong on the Mac';
}

/** One thing the assistant did: what, how it went, when, and how long it took. */
export const ActivityRow = memo(function ActivityRow({ item, fresh, now }: { item: Activity; fresh?: boolean; now: number }) {
  const outcome = OUTCOME[item.outcome];
  const took = item.outcome === 'pending' ? null : duration(item.duration_ms);
  const meta = [item.client, took].filter(Boolean).join(' · ');
  const note = item.outcome === 'error' ? failureText(item.error_code) : item.outcome === 'denied' ? 'Denied' : null;
  return (
    <li className={`${styles.row} ${fresh ? styles.fresh : ''}`}>
      <span className={`${styles.glyph} ${outcome.className}`} aria-hidden="true">
        <Icon name={outcome.icon} size={15} weight={2.4} />
      </span>
      <div className={styles.main}>
        <span className={`t-body ${styles.title}`}>{item.title}</span>
        <p className={`t-subhead secondary ${styles.sub}`}>
          <span className="visually-hidden">{outcome.label}. </span>
          <span className={styles.detail}>{item.detail}</span>
          {/* The dot stays with the detail, so a wrapped line never starts with one. */}
          <span className={styles.inlineMeta}>{item.detail && meta ? '\u00a0·' : ''}</span>{' '}
          <span className={styles.inlineMeta}>{meta}</span>
        </p>
        {note ? (
          <p className={`t-footnote ${item.outcome === 'error' ? styles.errorText : 'text-orange'}`} title={item.error_code ?? undefined}>
            {note}
          </p>
        ) : null}
      </div>
      <span className={`t-subhead secondary ${styles.client}`}>{item.client}</span>
      <span className={`t-subhead secondary tabular ${styles.took}`}>{took}</span>
      <time className={`t-subhead secondary tabular ${styles.time}`} dateTime={item.at} title={stamp(item.at)}>
        {when(item.at, now)}
      </time>
    </li>
  );
});

