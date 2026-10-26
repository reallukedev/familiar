import { memo, type ReactNode } from 'react';
import { bytes } from '../../lib/format';
import { fileType, finderDate, splitMiddle } from '../../lib/files';
import type { FileItem } from '../../lib/types';
import { Icon } from '../Icon';
import { ActionMenu } from '../Menu';
import { Section } from '../List';
import { SkeletonRows } from '../Skeleton';
import { FileIcon } from './FileIcon';
import styles from './PhoneFiles.module.css';

type Props = {
  entries: FileItem[] | null;
  footer: ReactNode;
  onOpen: (item: FileItem) => void;
  onDownload: (item: FileItem) => void;
  onTrash: (item: FileItem) => void;
  onQuickLook: (item: FileItem) => void;
};

/** The Files app's list: big icons, name over date and size, a chevron into folders. */
export function PhoneFiles({ entries, footer, onOpen, onDownload, onTrash, onQuickLook }: Props) {
  if (!entries) {
    return (
      <div className={styles.loading}>
        <SkeletonRows rows={7} leading label="Loading files" />
      </div>
    );
  }
  const now = Date.now();
  return (
    <Section footer={footer}>
      {entries.map((item) => (
        <PhoneRow key={item.path} item={item} now={now} onOpen={onOpen} onDownload={onDownload} onTrash={onTrash} onQuickLook={onQuickLook} />
      ))}
    </Section>
  );
}

const PhoneRow = memo(function PhoneRow({ item, now, onOpen, onDownload, onTrash, onQuickLook }: { item: FileItem; now: number } & Omit<Props, 'entries' | 'footer'>) {
  const type = fileType(item);
  const name = splitMiddle(item.name, 7);
  const subtitle = [finderDate(item.modified, now), item.kind === 'dir' ? null : bytes(item.size)].filter(Boolean).join(' · ');
  const body = (
    <>
      <FileIcon item={item} size={36} />
      <span className={styles.text}>
        <span className={styles.name}>
          <span className={styles.head}>{name.head}</span>
          {name.tail ? <span className={styles.tail}>{name.tail}</span> : null}
        </span>
        <span className={styles.subtitle}>{subtitle}</span>
      </span>
    </>
  );
  const items = [
    ...(type.navigable ? [{ label: 'Open', onSelect: () => onOpen(item) }] : [{ label: 'Quick Look', onSelect: () => onQuickLook(item) }]),
    { label: 'Download', onSelect: () => onDownload(item), disabled: !type.downloadable },
    { label: 'Move to Trash', onSelect: () => onTrash(item), tone: 'destructive' as const, separated: true },
  ];

  if (type.navigable) {
    return (
      <div className={styles.row}>
        <button type="button" className={styles.main} onClick={() => onOpen(item)} aria-label={`${item.name}, folder`}>
          {body}
        </button>
        <ActionMenu
          label={`Actions for ${item.name}`}
          items={items}
          trigger={
            <button type="button" className={styles.more}>
              <Icon name="ellipsis" size={22} weight={3} />
            </button>
          }
        />
        {/* Part of the row's tap target; the row's button already names the action. */}
        <span className={styles.chevron} aria-hidden="true" onClick={() => onOpen(item)}>
          <Icon name="chevron-right" size={16} weight={2.4} />
        </span>
      </div>
    );
  }
  // A file's row is its menu: tap it for Quick Look, Download or Move to Trash, as a long press would in Files.
  return (
    <div className={styles.row}>
      <ActionMenu
        label={`${item.name}, ${type.label}`}
        items={items}
        trigger={
          <button type="button" className={styles.main}>
            {body}
            <span className={styles.moreGlyph} aria-hidden="true">
              <Icon name="ellipsis" size={22} weight={3} />
            </span>
          </button>
        }
      />
    </div>
  );
});
