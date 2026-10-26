import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import { expiresIn, whenInline } from '../lib/format';
import { putApproval } from '../lib/live';
import { usePlatform } from '../lib/platform';
import { notify } from '../lib/toast';
import type { Approval } from '../lib/types';
import { Button } from './Button';
import { Familiar } from './Familiar';
import styles from './Approvals.module.css';

function useNow(active: boolean, everyMs = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(timer);
  }, [active, everyMs]);
  return now;
}

/**
 * Requests waiting for the owner. Prominent, live, and answerable in place.
 * Announced to screen readers as they arrive.
 */
export function ApprovalList({ approvals, macName }: { approvals: Approval[]; macName?: string }) {
  const pending = approvals.filter((item) => item.status === 'pending');
  const now = useNow(pending.length > 0);
  // A Mac shows each request as a macOS alert; a phone as a notification.
  const variant = usePlatform() === 'phone' ? 'notification' : 'alert';
  return (
    <div className={styles.list} data-cards={variant} aria-live="polite" aria-relevant="additions">
      {pending.map((approval) => (
        <ApprovalCard key={approval.id} approval={approval} now={now} macName={macName} variant={variant} />
      ))}
    </div>
  );
}

function ApprovalCard({ approval, now, macName, variant }: { approval: Approval; now: number; macName?: string; variant: 'notification' | 'alert' }) {
  const [busy, setBusy] = useState<'approve' | 'deny' | null>(null);
  const expired = new Date(approval.expires_at).getTime() <= now;

  async function decide(decision: 'approve' | 'deny') {
    setBusy(decision);
    try {
      const updated = await api.decide(approval.instance_id, approval.id, decision);
      putApproval(updated);
      notify.success(decision === 'approve' ? 'Allowed' : 'Denied');
    } catch (error) {
      notify.error(error);
    } finally {
      setBusy(null);
    }
  }

  const left = new Date(approval.expires_at).getTime() - now;
  const who = [approval.client ?? 'Your assistant', macName].filter(Boolean).join(' on ');

  const countdown = (
    <span className={`tabular ${styles.when} ${!expired && left < 60_000 ? styles.soon : ''}`} title={`Asked ${whenInline(approval.created_at, now)}`}>
      {expired ? 'Expired' : expiresIn(approval.expires_at, now)}
    </span>
  );

  if (variant === 'alert') {
    // A macOS alert: app icon, the question, the request, then capsule buttons.
    return (
      <article className={styles.alert} aria-labelledby={`approval-${approval.id}`}>
        <div className={styles.alertBody}>
          <span className={styles.alertIcon} aria-hidden="true">
            <Familiar state="attention" size={48} />
          </span>
          <h3 className={styles.alertTitle} id={`approval-${approval.id}`}>
            Allow {approval.client ?? 'your assistant'} to {lowerFirst(approval.title)}?
          </h3>
          <p className={styles.alertMessage}>{approval.summary}</p>
          <ApprovalDetails args={approval.arguments} summary={approval.summary} />
          <p className={styles.alertMeta}>
            {macName ? `${macName} · ` : ''}
            {countdown}
          </p>
        </div>
        <div className={styles.alertButtons}>
          <Button variant="gray" onClick={() => decide('deny')} busy={busy === 'deny'} disabled={!!busy || expired}>
            Don’t Allow
          </Button>
          <Button variant="filled" onClick={() => decide('approve')} busy={busy === 'approve'} disabled={!!busy || expired}>
            Allow
          </Button>
        </div>
      </article>
    );
  }

  return (
    <article className={styles.card} aria-labelledby={`approval-${approval.id}`}>
      <span className={styles.icon} aria-hidden="true">
        <Familiar state="attention" size={30} />
      </span>
      <div className={styles.text}>
        <p className={styles.meta}>
          <span className={styles.who}>{who}</span>
          {countdown}
        </p>
        {/* The summary already names the action, so it is the title. */}
        <h3 className={styles.title} id={`approval-${approval.id}`}>
          {approval.summary}
        </h3>
        <ApprovalDetails args={approval.arguments} summary={approval.summary} />
      </div>
      <div className={styles.actions}>
        <Button variant="gray" onClick={() => decide('deny')} busy={busy === 'deny'} disabled={!!busy || expired}>
          Deny
        </Button>
        <Button variant="filled" onClick={() => decide('approve')} busy={busy === 'approve'} disabled={!!busy || expired}>
          Allow
        </Button>
      </div>
    </article>
  );
}

// Shown so nothing is approved unseen: recipients, attachments, flags. request_id is noise.
const HIDDEN = new Set(['request_id']);

function formatValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(formatValue).join(', ');
  if (value !== null && typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/**
 * What the request will actually do, shown in place so nothing is approved
 * unseen. Values the summary already spells out are left out, so a simple
 * request stays one line and an email still shows its body.
 */
function ApprovalDetails({ args, summary }: { args?: Record<string, unknown>; summary: string }) {
  const entries = Object.entries(args ?? {})
    .filter(([key, value]) => !HIDDEN.has(key) && value !== null && value !== '' && !(Array.isArray(value) && value.length === 0))
    .map(([key, value]) => [key, formatValue(value)] as const)
    .filter(([, text]) => !summary.includes(text));
  if (entries.length === 0) return null;
  return (
    <dl className={styles.args}>
      {entries.map(([key, text]) => (
        <div key={key} className={styles.arg}>
          <dt>{label(key)}</dt>
          <dd className={styles.value} title={text}>
            {text}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function label(key: string) {
  const words = key.replaceAll('_', ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function lowerFirst(text: string) {
  return text.charAt(0).toLowerCase() + text.slice(1);
}
