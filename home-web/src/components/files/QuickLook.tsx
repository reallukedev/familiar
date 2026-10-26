import { Dialog } from '@base-ui/react/dialog';
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { bytes, count } from '../../lib/format';
import { fileType, finderDate } from '../../lib/files';
import type { FileItem } from '../../lib/types';
import { Button } from '../Button';
import { Icon } from '../Icon';
import { Spinner } from '../Spinner';
import { FileIcon } from './FileIcon';
import { useFolderSize, type SizeTotal } from './folderSize';
import { TEXT_LIMIT, useMediaFallback, usePreview, type Preview } from './preview';
import styles from './QuickLook.module.css';

type Props = {
  instanceId: string;
  macName: string;
  /** The Mac is up; previews need it. */
  running: boolean;
  /** What's being looked at; null closes Quick Look. */
  item: FileItem | null;
  onClose: () => void;
  onDownload: (item: FileItem) => void;
};

type MacProps = Props & {
  /** ↑ and ↓ move the list's selection; the preview follows. */
  onMove: (delta: 1 | -1) => void;
  /** The row Quick Look zooms out of and back into. */
  anchor: () => HTMLElement | null;
  /** Where focus goes when it closes (the list). */
  returnFocus: () => HTMLElement | null;
};

/** The item stays on screen while the panel animates closed. */
function useLast(item: FileItem | null): FileItem | null {
  const [last, setLast] = useState(item);
  if (item && item !== last) setLast(item);
  return item ?? last;
}

/** Folder sizes worked out while Quick Look is open; forgotten when it closes. */
function useSizeCache(open: boolean): Map<string, SizeTotal> {
  const cache = useRef(new Map<string, SizeTotal>());
  useEffect(() => {
    if (!open) cache.current.clear();
  }, [open]);
  return cache.current;
}

type Layout = 'card' | 'fit' | 'fill';

function layoutOf(preview: Preview, broken: boolean): Layout {
  if (broken) return 'card';
  switch (preview.state) {
    case 'image':
      return 'fit';
    case 'pdf':
    case 'text':
    case 'video':
      return 'fill';
    default:
      return 'card';
  }
}

/** macOS Quick Look: a floating panel that zooms out of the selected row. */
export function QuickLook({ instanceId, macName, running, item, onClose, onDownload, onMove, anchor, returnFocus }: MacProps) {
  const shown = useLast(item);
  const [preview, retry] = usePreview(instanceId, item, running);
  const media = useMediaFallback(previewUrl(preview));
  const sizes = useSizeCache(!!item);
  const popup = useRef<HTMLDivElement | null>(null);
  const layout = layoutOf(preview, media.broken);

  // Zoom from the row: the transform origin sits on the row's icon, in the panel's own coordinates.
  const aim = useCallback(() => {
    const panel = popup.current;
    const row = anchor();
    if (!panel || !row) return;
    const rect = row.getBoundingClientRect();
    const x = rect.left + Math.min(28, rect.width / 2) - panel.offsetLeft;
    const y = rect.top + rect.height / 2 - panel.offsetTop;
    panel.style.transformOrigin = `${Math.round(x)}px ${Math.round(y)}px`;
  }, [anchor]);

  const attach = useCallback(
    (node: HTMLDivElement | null) => {
      popup.current = node;
      if (node) aim();
    },
    [aim],
  );

  // The panel resizes to each file; keep the origin on the selected row.
  useEffect(() => {
    if (item) requestAnimationFrame(aim);
  }, [item, layout, aim]);

  const close = useCallback(() => {
    aim();
    onClose();
  }, [aim, onClose]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    // Media controls keep their own keys (Space plays and pauses).
    if ((event.target as HTMLElement).closest('video, audio')) return;
    if (event.key === ' ') {
      event.preventDefault();
      close();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      onMove(event.key === 'ArrowDown' ? 1 : -1);
    }
  }

  const type = shown ? fileType(shown) : null;

  return (
    <Dialog.Root open={!!item} onOpenChange={(next) => !next && close()}>
      <Dialog.Portal>
        <Dialog.Backdrop className={styles.backdrop} />
        <Dialog.Viewport className={styles.viewport}>
          <Dialog.Popup
            ref={attach}
            className={styles.panel}
            data-layout={layout}
            initialFocus={popup}
            finalFocus={() => returnFocus() ?? true}
            onKeyDown={onKeyDown}
          >
            <div className={styles.titleBar}>
              <Dialog.Close className={styles.close} aria-label="Close">
                <Icon name="xmark" size={10} weight={2.6} />
              </Dialog.Close>
              <Dialog.Title className={styles.title} title={shown?.name}>
                {shown ? <FileIcon item={shown} size={16} /> : null}
                <span>{shown?.name}</span>
              </Dialog.Title>
              {shown && type?.downloadable && running ? (
                <button type="button" className={styles.openWith} onClick={() => onDownload(shown)}>
                  <Icon name="download" size={14} weight={2} />
                  Download
                </button>
              ) : (
                <span />
              )}
            </div>
            {shown ? <PreviewBody item={shown} preview={preview} media={media} macName={macName} onDownload={onDownload} onRetry={retry} instanceId={instanceId} sizes={sizes} /> : null}
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** iOS Quick Look: a full-screen sheet with Done and a download button. */
export function PhoneQuickLook({ instanceId, macName, running, item, onClose, onDownload }: Props) {
  const shown = useLast(item);
  const [preview, retry] = usePreview(instanceId, item, running);
  const media = useMediaFallback(previewUrl(preview));
  const sizes = useSizeCache(!!item);
  const type = shown ? fileType(shown) : null;
  return (
    <Dialog.Root open={!!item} onOpenChange={(next) => !next && onClose()}>
      <Dialog.Portal>
        <Dialog.Popup className={styles.phone} data-layout={layoutOf(preview, media.broken)}>
          <div className={styles.phoneBar}>
            <Dialog.Close className={styles.phoneDone}>Done</Dialog.Close>
            <Dialog.Title className={styles.phoneTitle}>{shown?.name}</Dialog.Title>
            {shown && type?.downloadable && running ? (
              <button type="button" className={styles.phoneAction} onClick={() => onDownload(shown)} aria-label="Download">
                <Icon name="download" size={22} weight={2} />
              </button>
            ) : (
              <span />
            )}
          </div>
          {shown ? <PreviewBody item={shown} preview={preview} media={media} macName={macName} onDownload={onDownload} onRetry={retry} instanceId={instanceId} sizes={sizes} phone /> : null}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function previewUrl(preview: Preview): string | null {
  return 'url' in preview ? preview.url : null;
}

type BodyProps = {
  item: FileItem;
  preview: Preview;
  media: ReturnType<typeof useMediaFallback>;
  macName: string;
  onDownload: (item: FileItem) => void;
  onRetry: () => void;
  instanceId: string;
  sizes: Map<string, SizeTotal>;
  phone?: boolean;
};

function PreviewBody({ item, preview, media, macName, onDownload, onRetry, instanceId, sizes, phone }: BodyProps) {
  const download = fileType(item).downloadable ? (
    <Button variant={phone ? 'tinted' : 'gray'} size={phone ? 'regular' : undefined} onClick={() => onDownload(item)}>
      Download
    </Button>
  ) : null;

  if (media.broken) return <Card item={item} action={download} />;

  switch (preview.state) {
    case 'loading':
      return (
        <div className={styles.card} aria-busy="true">
          <div className={styles.loading}>
            <span className={styles.faint}>
              <FileIcon item={item} size={phone ? 112 : 128} />
            </span>
            <span className={styles.spinner}>
              <Spinner size={phone ? 28 : 24} label="Loading preview" />
            </span>
          </div>
        </div>
      );
    case 'image':
      return (
        <div className={styles.image}>
          <img src={preview.url} alt={item.name} onError={media.fail} draggable={false} />
        </div>
      );
    case 'pdf':
      return <iframe className={styles.pdf} src={preview.url} title={item.name} />;
    case 'text':
      return (
        <div className={styles.textPane}>
          <pre className={`${styles.text} ${preview.code ? styles.code : ''}`} tabIndex={0} aria-label={`Contents of ${item.name}`}>
            {preview.text}
          </pre>
          {preview.truncated ? (
            <p className={styles.truncated}>
              Showing the first {bytes(TEXT_LIMIT)}.{download ? ' Download the file to see all of it.' : null}
            </p>
          ) : null}
        </div>
      );
    case 'video':
      return (
        <div className={styles.video}>
          <video src={preview.url} controls autoPlay playsInline onError={media.fail} aria-label={item.name} />
        </div>
      );
    case 'audio':
      return (
        <Card item={item}>
          <audio className={styles.audio} src={preview.url} controls autoPlay onError={media.fail} aria-label={item.name} />
        </Card>
      );
    case 'info':
      return item.kind === 'dir' ? (
        <Card item={item}>
          <FolderSize key={item.path} instanceId={instanceId} path={item.path} sizes={sizes} />
        </Card>
      ) : (
        <Card item={item} action={download} />
      );
    case 'large':
      return <Card item={item} note="This file is too large to preview." action={download} />;
    case 'failed':
      return (
        <Card
          item={item}
          note="Couldn’t load the preview."
          detail={preview.message}
          action={
            <>
              <Button variant="plain" onClick={onRetry}>
                Try Again
              </Button>
              {download}
            </>
          }
        />
      );
    case 'offline':
      return <Card item={item} faint note={`${macName} isn’t running.`} detail="Start it to preview its files." />;
  }
}

function Card({ item, note, detail, action, faint, children }: { item: FileItem; note?: string; detail?: string; action?: ReactNode; faint?: boolean; children?: ReactNode }) {
  const type = fileType(item);
  const folder = item.kind === 'dir';
  const facts = [type.label, folder ? null : bytes(item.size)].filter(Boolean).join(' · ');
  return (
    <div className={styles.card}>
      <span className={faint ? styles.faint : undefined}>
        <FileIcon item={item} size={128} />
      </span>
      <div className={styles.facts}>
        <p className={styles.name}>{item.name}</p>
        <p className={styles.meta}>{facts}</p>
        {item.modified ? <p className={styles.meta}>Modified {finderDate(item.modified)}</p> : null}
      </div>
      {note ? (
        <div className={styles.note} role={note && !faint ? 'status' : undefined}>
          <p>{note}</p>
          {detail ? <p className={styles.detail}>{detail}</p> : null}
        </div>
      ) : null}
      {children}
      {action ? <div className={styles.actions}>{action}</div> : null}
    </div>
  );
}

/** Finder's Calculate Size: walks the folder and keeps a running total. */
function FolderSize({ instanceId, path, sizes }: { instanceId: string; path: string; sizes: Map<string, SizeTotal> }) {
  const { size, start, cancel } = useFolderSize(instanceId, path, sizes);
  const total = (value: SizeTotal) => `${bytes(value.bytes)}, ${count(value.items, 'item')}`;
  switch (size.state) {
    case 'idle':
      return (
        <Button variant="gray" onClick={start}>
          Calculate Size
        </Button>
      );
    case 'running':
      return (
        <div className={styles.sizing}>
          <p className={styles.meta} role="status">
            <Spinner size={13} /> Calculating… {size.items ? total(size) : null}
          </p>
          <Button variant="plain" onClick={cancel}>
            Cancel
          </Button>
        </div>
      );
    case 'done':
      return (
        <p className={styles.sizeResult} role="status">
          {size.atLeast ? `At least ${total(size)}` : total(size)}
        </p>
      );
    case 'failed':
      return (
        <div className={styles.sizing}>
          <p className={styles.meta} role="status">
            Couldn’t calculate the size. {size.message}
          </p>
          <Button variant="plain" onClick={start}>
            Try Again
          </Button>
        </div>
      );
  }
}
