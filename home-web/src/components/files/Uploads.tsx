import type { CSSProperties } from 'react';
import { bytes, count } from '../../lib/format';
import { splitMiddle } from '../../lib/files';
import { Button, IconButton } from '../Button';
import { Icon } from '../Icon';
import { Section } from '../List';
import { FileIcon } from './FileIcon';
import { LIMIT, type Upload } from './useUploads';
import styles from './Uploads.module.css';

type Props = {
  uploads: Upload[];
  onCancel: (key: string) => void;
  onRetry: (key: string) => void;
  onClear: () => void;
};

/** What's going up to the Mac, one row per file, with a bar while it runs. */
export function Uploads({ uploads, onCancel, onRetry, onClear }: Props) {
  if (!uploads.length) return null;
  const active = uploads.filter((item) => item.status === 'waiting' || item.status === 'uploading').length;
  const failed = uploads.filter((item) => item.status === 'failed').length;
  const title = active ? `Uploading ${count(active, 'item')}` : failed ? `${count(failed, 'upload')} didn’t finish` : 'Uploaded';
  return (
    <Section
      title={title}
      live="polite"
      accessory={
        active < uploads.length ? (
          <Button variant="plain" size="small" onClick={onClear}>
            Clear
          </Button>
        ) : undefined
      }
    >
      {uploads.map((item) => (
        <UploadRow key={item.key} item={item} onCancel={onCancel} onRetry={onRetry} />
      ))}
    </Section>
  );
}

function UploadRow({ item, onCancel, onRetry }: { item: Upload; onCancel: (key: string) => void; onRetry: (key: string) => void }) {
  const name = splitMiddle(item.file.name);
  const where = `“${item.folderName}”`;
  const status =
    item.status === 'waiting'
      ? `Waiting · ${bytes(item.file.size)}`
      : item.status === 'uploading'
        ? item.progress === null
          ? `Uploading to ${where} · ${bytes(item.file.size)}`
          : `Uploading to ${where} · ${bytes(Math.round(item.file.size * item.progress))} of ${bytes(item.file.size)}`
        : item.status === 'done'
          ? item.savedAs
            ? `Saved in ${where} as “${item.savedAs}”, because that name was taken`
            : `Uploaded to ${where}`
          : item.error;
  return (
    <div className={styles.row} data-status={item.status}>
      <FileIcon item={{ name: item.file.name, kind: 'file' }} size={28} />
      <div className={styles.text}>
        <span className={styles.name} title={item.file.name}>
          <span className={styles.head}>{name.head}</span>
          {name.tail ? <span className={styles.tail}>{name.tail}</span> : null}
        </span>
        {item.status === 'uploading' || item.status === 'waiting' ? (
          <span
            className={styles.track}
            role="progressbar"
            aria-label={`Uploading ${item.file.name}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={item.progress === null ? undefined : Math.round(item.progress * 100)}
          >
            <span
              className={`${styles.fill} ${item.progress === null && item.status === 'uploading' ? styles.indeterminate : ''}`}
              style={item.progress === null ? undefined : ({ '--progress': item.progress } as CSSProperties)}
            />
          </span>
        ) : null}
        <span className={`${styles.status} ${item.status === 'failed' ? styles.failed : ''}`}>
          {item.status === 'done' ? <Icon name="check" size={13} weight={2.6} className={styles.check} /> : null}
          {status}
        </span>
      </div>
      <div className={styles.actions}>
        {item.status === 'failed' && item.file.size <= LIMIT ? (
          <Button variant="gray" size="small" onClick={() => onRetry(item.key)}>
            Try Again
          </Button>
        ) : null}
        {item.status !== 'done' ? (
          <IconButton label={item.status === 'failed' ? `Remove ${item.file.name}` : `Stop uploading ${item.file.name}`} onClick={() => onCancel(item.key)}>
            <Icon name="xmark" size={14} weight={2.4} />
          </IconButton>
        ) : null}
      </div>
    </div>
  );
}
