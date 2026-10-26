import { ContextMenu } from '@base-ui/react/context-menu';
import { memo, useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode, type RefObject } from 'react';
import { bytes } from '../../lib/format';
import { fileType, finderDate, splitMiddle, typeSelect, type Sort, type SortKey } from '../../lib/files';
import { isMacKeyboard } from '../../lib/platform';
import type { FileItem } from '../../lib/types';
import { Icon } from '../Icon';
import menu from '../Menu.module.css';
import { FileIcon } from './FileIcon';
import styles from './FinderList.module.css';

const COLUMNS: { key: SortKey; label: string; className?: string }[] = [
  { key: 'name', label: 'Name' },
  { key: 'modified', label: 'Date Modified', className: styles.date },
  { key: 'size', label: 'Size', className: styles.size },
  { key: 'kind', label: 'Kind', className: styles.kind },
];

export type FinderActions = {
  onSelect: (path: string | null) => void;
  onOpen: (item: FileItem) => void;
  onUp: () => void;
  onDownload: (item: FileItem) => void;
  onTrash: (item: FileItem) => void;
  onNewFolder: () => void;
  onUpload: () => void;
  /** Space, or the context menu: Quick Look the item. */
  onQuickLook: (item: FileItem) => void;
};

type Props = FinderActions & {
  label: string;
  /** null while the folder loads. */
  entries: FileItem[] | null;
  sort: Sort;
  onSort: (key: SortKey) => void;
  selected: string | null;
  empty: ReactNode;
  status: ReactNode;
  dropping: boolean;
  /** The focusable list, for focus coming back from Quick Look. */
  gridRef?: RefObject<HTMLDivElement | null>;
};

/** Finder's list view: sortable columns, one selection, keyboard and a context menu. */
export function FinderList({ label, entries, sort, onSort, selected, empty, status, dropping, gridRef, ...actions }: Props) {
  const id = useId();
  const ownGrid = useRef<HTMLDivElement>(null);
  const grid = gridRef ?? ownGrid;
  const typed = useRef({ text: '', at: 0 });
  const [target, setTarget] = useState<FileItem | null>(null);
  const scrollOnSelect = useRef(false);
  const handlers = useRef(actions);
  useEffect(() => {
    handlers.current = actions;
  });
  const items = entries ?? [];
  const index = selected ? items.findIndex((item) => item.path === selected) : -1;
  const current = index >= 0 ? items[index] : null;
  const now = Date.now();

  useLayoutEffect(() => {
    if (!scrollOnSelect.current || index < 0) return;
    scrollOnSelect.current = false;
    document.getElementById(`${id}-${index}`)?.scrollIntoView({ block: 'nearest' });
  }, [index, id]);

  // A new folder starts with the list focused, so the keyboard works at once.
  useEffect(() => {
    // No ring for focus the person didn't move there themselves (focusVisible: false).
    if (entries && document.activeElement === document.body) grid.current?.focus({ preventScroll: true, focusVisible: false } as FocusOptions);
  }, [entries, label]);

  function move(to: number) {
    if (!items.length) return;
    const next = items[Math.max(0, Math.min(items.length - 1, to))];
    if (!next) return;
    scrollOnSelect.current = true;
    actions.onSelect(next.path);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.target !== event.currentTarget) return;
    const command = isMacKeyboard ? event.metaKey : event.ctrlKey;
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        if (command) {
          if (current) actions.onOpen(current);
        } else move(index < 0 ? 0 : index + 1);
        return;
      case 'ArrowUp':
        event.preventDefault();
        if (command) actions.onUp();
        else move(index < 0 ? items.length - 1 : index - 1);
        return;
      case 'Home':
        event.preventDefault();
        move(0);
        return;
      case 'End':
        event.preventDefault();
        move(items.length - 1);
        return;
      case 'Enter':
        if (current) {
          event.preventDefault();
          actions.onOpen(current);
        }
        return;
      case 'Backspace':
      case 'Delete':
        if (current && (command || event.key === 'Delete')) {
          event.preventDefault();
          actions.onTrash(current);
        }
        return;
      case 'Escape':
        if (current) actions.onSelect(null);
        return;
    }
    if (event.key.length !== 1 || event.metaKey || event.ctrlKey || event.altKey) return;
    const at = performance.now();
    const buffer = at - typed.current.at > 1000 ? '' : typed.current.text;
    if (event.key === ' ' && !buffer) {
      // Finder: Space is Quick Look unless it's part of a name being typed.
      event.preventDefault();
      if (current) actions.onQuickLook(current);
      return;
    }
    event.preventDefault();
    typed.current = { text: buffer + event.key, at };
    const hit = typeSelect(items, typed.current.text);
    if (hit !== null) move(hit);
  }

  function onContextMenuCapture(event: MouseEvent<HTMLDivElement>) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-path]');
    const item = row ? (items.find((entry) => entry.path === row.dataset.path) ?? null) : null;
    setTarget(item);
    actions.onSelect(item?.path ?? null);
    grid.current?.focus({ preventScroll: true });
  }

  const type = target ? fileType(target) : null;

  return (
    <div className={`${styles.box} ${dropping ? styles.dropping : ''}`}>
      <div
        ref={grid}
        className={styles.grid}
        role="grid"
        aria-label={label}
        aria-rowcount={items.length + 1}
        aria-multiselectable="false"
        aria-activedescendant={index >= 0 ? `${id}-${index}` : undefined}
        aria-busy={!entries || undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
      >
        <div className={styles.header} role="rowgroup">
          <div className={styles.row} role="row" aria-rowindex={1}>
            {COLUMNS.map((column) => {
              const active = sort.key === column.key;
              return (
                <div
                  key={column.key}
                  role="columnheader"
                  className={`${styles.column} ${column.className ?? ''}`}
                  aria-sort={active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : 'none'}
                >
                  <button type="button" className={`${styles.sort} ${active ? styles.sortActive : ''}`} onClick={() => onSort(column.key)} tabIndex={-1}>
                    <span>{column.label}</span>
                    <span className={styles.chevron} data-dir={sort.dir} aria-hidden="true">
                      {active ? <Icon name="chevron-down" size={10} weight={2.6} /> : null}
                    </span>
                  </button>
                </div>
              );
            })}
          </div>
        </div>
        <ContextMenu.Root>
          <ContextMenu.Trigger
            render={<div role="rowgroup" />}
            className={styles.body}
            onContextMenuCapture={onContextMenuCapture}
            onClick={(event: MouseEvent<HTMLDivElement>) => {
              if (event.target === event.currentTarget) actions.onSelect(null);
            }}
          >
            {!entries ? (
              <SkeletonRows />
            ) : items.length === 0 ? (
              <div className={styles.empty} role="row">
                <div role="gridcell">{empty}</div>
              </div>
            ) : (
              items.map((item, i) => (
                <Row key={item.path} item={item} index={i} id={`${id}-${i}`} selected={i === index} now={now} handlers={handlers} />
              ))
            )}
          </ContextMenu.Trigger>
          <ContextMenu.Portal>
            <ContextMenu.Positioner className={menu.positioner} collisionPadding={12}>
              <ContextMenu.Popup className={`${menu.popup} ${menu.mac}`}>
                {target && type ? (
                  <>
                    <MenuItem onClick={() => actions.onOpen(target)} disabled={!type.navigable && !type.downloadable}>
                      {type.navigable ? 'Open' : 'Download'}
                    </MenuItem>
                    <MenuItem onClick={() => actions.onQuickLook(target)} hint="Space">
                      Quick Look
                    </MenuItem>
                    <ContextMenu.Separator className={menu.separator} />
                    <MenuItem onClick={() => actions.onTrash(target)} hint={isMacKeyboard ? '⌘⌫' : 'Delete'} destructive>
                      Move to Trash
                    </MenuItem>
                  </>
                ) : (
                  <>
                    <MenuItem onClick={actions.onNewFolder}>New Folder</MenuItem>
                    <MenuItem onClick={actions.onUpload}>Upload Files…</MenuItem>
                  </>
                )}
              </ContextMenu.Popup>
            </ContextMenu.Positioner>
          </ContextMenu.Portal>
        </ContextMenu.Root>
      </div>
      <div className={styles.status} role="status">
        {status}
      </div>
    </div>
  );
}

function MenuItem({ children, onClick, disabled, hint, destructive }: { children: ReactNode; onClick: () => void; disabled?: boolean; hint?: string; destructive?: boolean }) {
  return (
    <ContextMenu.Item className={`${menu.item} ${destructive ? menu.destructive : ''}`} onClick={onClick} disabled={disabled}>
      <span className={menu.itemLabel}>{children}</span>
      {hint ? <kbd className={menu.hint}>{hint}</kbd> : null}
    </ContextMenu.Item>
  );
}

const Row = memo(function Row({ item, index, id, selected, now, handlers }: { item: FileItem; index: number; id: string; selected: boolean; now: number; handlers: { current: FinderActions } }) {
  const type = fileType(item);
  const name = splitMiddle(item.name);
  const folderLike = item.kind === 'dir';
  return (
    <div
      id={id}
      role="row"
      aria-rowindex={index + 2}
      aria-selected={selected}
      data-path={item.path}
      className={`${styles.row} ${styles.item} ${selected ? styles.selected : ''}`}
      onMouseDown={(event) => {
        if (event.button === 0) handlers.current.onSelect(item.path);
      }}
      onDoubleClick={() => handlers.current.onOpen(item)}
    >
      <div role="gridcell" className={styles.nameCell} title={item.name}>
        <FileIcon item={item} size={18} />
        <span className={styles.name}>
          <span className={styles.head}>{name.head}</span>
          {name.tail ? <span className={styles.tail}>{name.tail}</span> : null}
        </span>
      </div>
      <div role="gridcell" className={`${styles.cell} ${styles.date}`}>
        {finderDate(item.modified, now)}
      </div>
      <div role="gridcell" className={`${styles.cell} ${styles.size}`}>
        {folderLike ? '--' : bytes(item.size)}
      </div>
      <div role="gridcell" className={`${styles.cell} ${styles.kind}`}>
        {type.label}
      </div>
    </div>
  );
});

function SkeletonRows() {
  return (
    <>
      {Array.from({ length: 9 }, (_, index) => (
        <div className={`${styles.row} ${styles.item} ${styles.skeleton}`} key={index} aria-hidden="true">
          <div className={styles.nameCell}>
            <span className={styles.boneIcon} />
            <span className={styles.bone} style={{ width: `${34 + ((index * 17) % 40)}%` }} />
          </div>
          <div className={`${styles.cell} ${styles.date}`}>
            <span className={styles.bone} style={{ width: '70%' }} />
          </div>
          <div className={`${styles.cell} ${styles.size}`}>
            <span className={styles.bone} style={{ width: '50%' }} />
          </div>
          <div className={`${styles.cell} ${styles.kind}`}>
            <span className={styles.bone} style={{ width: `${40 + ((index * 23) % 40)}%` }} />
          </div>
        </div>
      ))}
    </>
  );
}
