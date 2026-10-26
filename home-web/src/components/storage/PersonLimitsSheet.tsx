import { useId, useState, type FormEvent, type ReactNode } from 'react';
import { CellBody, Row, Section } from '../List';
import { Segmented, type SegmentOption } from '../Segmented';
import { Sheet } from '../Sheet';
import { bytes, count } from '../../lib/format';
import {
  GB, LIMIT_KEYS, draftFromOverride, limitLabel, parseWhole, readOverrides, type LimitKey, type OverrideDraft,
} from '../../lib/storage';
import type { PersonStorage, StorageDefaults, StorageOverrides } from '../../lib/types';
import { LimitInput } from './LimitInput';
import styles from './PersonLimitsSheet.module.css';

type Mode = 'default' | 'custom' | 'none';
type Drafts = Record<LimitKey, OverrideDraft>;

const TITLE: Record<LimitKey, string> = {
  quota_gb: 'Storage limit',
  max_disk_gb: 'Largest disk for a new Mac',
  max_snapshots: 'Snapshots per Mac',
};

const UNIT: Record<LimitKey, string | undefined> = { quota_gb: 'GB', max_disk_gb: 'GB', max_snapshots: undefined };

type Props = {
  person: PersonStorage | null;
  defaults: StorageDefaults;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Resolves true once saved; the sheet closes then. On false it stays open with what was typed. */
  onSave: (person: PersonStorage, body: Partial<StorageOverrides>) => Promise<boolean>;
  /** A server rejection for one limit, shown under it. */
  serverError?: { key: LimitKey; message: string } | null;
};

/** Server › Storage: one person's limits, each the server default, their own value, or no limit. */
export function PersonLimitsSheet({ person, defaults, open, onOpenChange, onSave, serverError }: Props) {
  const form = useId();
  const [busy, setBusy] = useState(false);
  return (
    <Sheet
      open={open && !!person}
      onOpenChange={(next) => {
        if (!busy || next) onOpenChange(next);
      }}
      title={person ? `Limits for ${person.display_name}` : 'Limits'}
      description={person ? usage(person) : undefined}
      primary={{ label: 'Save', form, busy }}
    >
      {person ? (
        <Body
          // Fresh drafts each time a person's sheet opens.
          key={`${person.user_id}:${open}`}
          form={form}
          person={person}
          defaults={defaults}
          busy={busy}
          serverError={serverError ?? null}
          onSubmit={async (body) => {
            if (!Object.keys(body).length) {
              onOpenChange(false);
              return;
            }
            setBusy(true);
            try {
              if (await onSave(person, body)) onOpenChange(false);
            } finally {
              setBusy(false);
            }
          }}
        />
      ) : null}
    </Sheet>
  );
}

function usage(person: PersonStorage): string {
  const macs = count(person.mac_count, 'Mac');
  return person.used_bytes > 0 ? `Uses ${bytes(person.used_bytes)} across ${macs}.` : person.mac_count ? `${macs}, nothing stored yet.` : 'No Macs yet.';
}

function modeOf(draft: OverrideDraft): Mode {
  if (!draft.custom) return 'default';
  return draft.unlimited ? 'none' : 'custom';
}

function Body({
  form, person, defaults, busy, serverError, onSubmit,
}: {
  form: string;
  person: PersonStorage;
  defaults: StorageDefaults;
  busy: boolean;
  serverError: { key: LimitKey; message: string } | null;
  onSubmit: (body: Partial<StorageOverrides>) => Promise<void>;
}) {
  const [drafts, setDrafts] = useState<Drafts>(() => ({
    quota_gb: draftFromOverride('quota_gb', person.overrides.quota_gb, defaults.quota_gb),
    max_disk_gb: draftFromOverride('max_disk_gb', person.overrides.max_disk_gb, defaults.max_disk_gb),
    max_snapshots: draftFromOverride('max_snapshots', person.overrides.max_snapshots, defaults.max_snapshots),
  }));
  const [attempted, setAttempted] = useState(false);
  const [edited, setEdited] = useState<Set<LimitKey>>(new Set());
  const result = readOverrides(drafts, person.overrides);
  const errors = 'errors' in result ? result.errors : {};

  const change = (key: LimitKey, patch: Partial<OverrideDraft>) => {
    setDrafts((prev) => ({ ...prev, [key]: { ...prev[key], ...patch } }));
    setEdited((prev) => new Set(prev).add(key));
  };

  function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setAttempted(true);
    if ('errors' in result) {
      const first = LIMIT_KEYS.find((key) => errors[key]);
      if (first) document.getElementById(`${form}-${first}`)?.focus();
      return;
    }
    void onSubmit(result.body);
  }

  const shownError = (key: LimitKey) => (attempted ? errors[key] : undefined) ?? (serverError?.key === key && !edited.has(key) ? serverError.message : undefined);

  const quotaText = drafts.quota_gb.custom && !drafts.quota_gb.unlimited ? parseWhole(drafts.quota_gb.text) : null;
  const quota = drafts.quota_gb.custom ? (drafts.quota_gb.unlimited ? 0 : quotaText) : defaults.quota_gb;
  const overAtOnce = quota !== null && quota > 0 && person.used_bytes >= quota * GB;

  return (
    <form id={form} onSubmit={submit} noValidate className={styles.form}>
      {LIMIT_KEYS.map((key) => (
        <LimitBlock
          key={key}
          id={`${form}-${key}`}
          limit={key}
          draft={drafts[key]}
          fallback={defaults[key]}
          disabled={busy}
          error={shownError(key)}
          onChange={(patch) => change(key, patch)}
          footer={footerFor(key, person, overAtOnce, quota)}
        />
      ))}
    </form>
  );
}

function footerFor(key: LimitKey, person: PersonStorage, overAtOnce: boolean, quota: number | null): ReactNode {
  const first = person.display_name.split(/\s+/)[0] || person.username;
  if (key === 'quota_gb') {
    if (overAtOnce && quota !== null) {
      return (
        <span className={styles.warn}>
          {first} already uses {bytes(person.used_bytes)}, so they’ll be over right away. Nothing is deleted.
        </span>
      );
    }
    return 'At the limit, they can’t create Macs or take snapshots.';
  }
  if (key === 'max_disk_gb') return 'Applies to new Macs.';
  return 'Existing snapshots are kept.';
}

function LimitBlock({
  id, limit, draft, fallback, disabled, error, onChange, footer,
}: {
  id: string;
  limit: LimitKey;
  draft: OverrideDraft;
  fallback: number;
  disabled: boolean;
  error: string | undefined;
  onChange: (patch: Partial<OverrideDraft>) => void;
  footer: ReactNode;
}) {
  const unlimitable = limit !== 'max_disk_gb';
  const mode = modeOf(draft);
  const errorId = `${id}-error`;
  const labelId = `${id}-label`;
  const options: SegmentOption<Mode>[] = [
    { value: 'default', label: 'Default' },
    { value: 'custom', label: 'Custom' },
    ...(unlimitable ? [{ value: 'none' as const, label: 'No Limit' }] : []),
  ];

  function pick(next: Mode) {
    if (next === 'default') onChange({ custom: false });
    else if (next === 'none') onChange({ custom: true, unlimited: true });
    else onChange({ custom: true, unlimited: false, text: draft.text || (fallback > 0 ? String(fallback) : '') });
  }

  return (
    <Section title={TITLE[limit]} footer={footer}>
      <CellBody>
        <Segmented label={TITLE[limit]} value={mode} options={options} onChange={pick} disabled={disabled} />
      </CellBody>
      {mode === 'default' ? (
        <Row title="Server default" value={limitLabel(limit, fallback)} />
      ) : mode === 'none' ? (
        <Row title="No limit" subtitle={limit === 'quota_gb' ? 'They can use as much as the server has free.' : 'They can keep as many as their storage allows.'} />
      ) : (
        <Row
          title={<span id={labelId}>{limit === 'max_snapshots' ? 'Snapshots' : 'Limit'}</span>}
          subtitle={error ? <span id={errorId} className={styles.error} role="alert">{error}</span> : undefined}
          align={error ? 'top' : 'center'}
          trailing={
            <LimitInput
              id={id}
              value={draft.text}
              onValueChange={(text) => onChange({ text })}
              unit={UNIT[limit]}
              invalid={!!error}
              disabled={disabled}
              aria-labelledby={labelId}
              aria-describedby={error ? errorId : undefined}
              placeholder={limit === 'max_snapshots' ? '10' : '500'}
            />
          }
        />
      )}
    </Section>
  );
}
