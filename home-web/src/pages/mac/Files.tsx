import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from 'react';
import { EmptyState } from '../../components/Bits';
import { Button, IconButton } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { FileIcon } from '../../components/files/FileIcon';
import { FinderList } from '../../components/files/FinderList';
import { NewFolderSheet } from '../../components/files/NewFolderSheet';
import { PhoneFiles } from '../../components/files/PhoneFiles';
import { PhoneQuickLook, QuickLook } from '../../components/files/QuickLook';
import { Uploads } from '../../components/files/Uploads';
import { downloadFile } from '../../components/files/download';
import { placeInListing, useUploads } from '../../components/files/useUploads';
import { Icon } from '../../components/Icon';
import { ActionMenu } from '../../components/Menu';
import { api, type ApiError } from '../../lib/api';
import { keys, updateData, useQuery } from '../../lib/cache';
import { crumbs, fileType, homeOf, nextSort, parentOf, sortEntries, toParam, type Sort, type SortKey } from '../../lib/files';
import { count } from '../../lib/format';
import { usePlatform } from '../../lib/platform';
import { navigate, onLinkClick, useSearchParam } from '../../lib/router';
import { macPath } from '../../lib/routes';
import { stateInfo } from '../../lib/status';
import { notify } from '../../lib/toast';
import type { FileItem, FolderListing, Instance } from '../../lib/types';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import { usePower } from './usePower';
import styles from './Files.module.css';

const SORT_STORE = 'dot-home:files-sort';
const BY_NAME: Sort = { key: 'name', dir: 'asc' };
const SORT_KEYS: readonly SortKey[] = ['name', 'modified', 'size', 'kind'];

function loadSort(): Sort {
  try {
    const saved = JSON.parse(localStorage.getItem(SORT_STORE) ?? 'null') as Sort | null;
    if (saved && SORT_KEYS.includes(saved.key) && (saved.dir === 'asc' || saved.dir === 'desc')) return saved;
  } catch {
    // Storage blocked or unreadable: Finder's default.
  }
  return BY_NAME;
}

function saveSort(sort: Sort) {
  try {
    localStorage.setItem(SORT_STORE, JSON.stringify(sort));
  } catch {
    // Not remembered this time; the list still sorts.
  }
}

const running = (instance: Instance) => instance.state === 'ready' || instance.state === 'degraded';

export function FilesPage({ instance }: MacPageProps) {
  const phone = usePlatform() === 'phone';
  const base = macPath(instance.id, 'files');
  const raw = useSearchParam('path');
  const path = raw ? toParam(raw, null) : '~';
  const hrefFor = useCallback((to: string) => (to === '~' ? base : `${base}?path=${encodeURIComponent(to).replace(/%2F/g, '/')}`), [base]);

  const listing = useQuery<FolderListing>(keys.files(instance.id, path), () => api.files(instance.id, path));
  const data = listing.data;
  const home = data ? homeOf(data.path) : null;
  const trail = crumbs(path);
  const folderName = trail.at(-1)!.label;
  const parent = parentOf(path);

  const [sort, setSort] = useState(loadSort);
  const entries = useMemo(() => (data ? sortEntries(data.entries, phone ? BY_NAME : sort) : null), [data, sort, phone]);
  const [selection, setSelection] = useState<{ folder: string; path: string | null }>({ folder: path, path: null });
  const selected = selection.folder === path ? selection.path : null;
  const [trashing, setTrashing] = useState<FileItem | null>(null);
  const [creating, setCreating] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [looking, setLooking] = useState<{ folder: string; path: string } | null>(null);
  const grid = useRef<HTMLDivElement>(null);
  const dragDepth = useRef(0);
  const picker = useRef<HTMLInputElement>(null);
  const uploads = useUploads(instance.id);
  const power = usePower(instance);

  const error = listing.error;
  const offline = error?.status === 503 || error?.code === 'unavailable';
  const failed = !!error && !data;
  const ready = !!data && !offline && !failed;

  // The Mac came up while this page was waiting on it: load the folder now.
  const on = running(instance);
  useEffect(() => {
    if (on && offline) void listing.reload();
  }, [on]);

  const select = useCallback((to: string | null) => setSelection({ folder: path, path: to }), [path]);

  const download = useCallback(
    (item: FileItem) => {
      downloadFile(instance.id, item).catch((cause: unknown) => notify.error(cause, 'Couldn’t download the file. Try again.'));
    },
    [instance.id],
  );

  const open = useCallback(
    (item: FileItem) => {
      const type = fileType(item);
      if (type.navigable) navigate(hrefFor(toParam(item.path, home)));
      else if (type.downloadable) download(item);
      else notify.info(`“${item.name}” opens on the Mac, not in the browser.`);
    },
    [hrefFor, home, download],
  );

  const goUp = useCallback(() => {
    if (!parent) return;
    // Like Finder, the folder you came out of stays selected.
    setSelection({ folder: parent, path: data?.path ?? null });
    navigate(hrefFor(parent), { direction: 'back' });
  }, [parent, data?.path, hrefFor]);

  const startUpload = useCallback(() => picker.current?.click(), []);
  const startFolder = useCallback(() => setCreating(true), []);

  async function trash(item: FileItem) {
    try {
      await api.trashFile(instance.id, item.path);
      updateData<FolderListing>(keys.files(instance.id, path), (current) => ({ ...current, entries: current.entries.filter((entry) => entry.path !== item.path) }));
      if (selected === item.path) select(null);
      notify.success(`Moved “${item.name}” to the Trash`);
    } catch (cause) {
      notify.error(cause);
    }
  }

  function addFiles(files: File[]) {
    if (files.length) uploads.add(files, path, folderName);
  }

  // Drag and drop: the whole content area takes files; the list shows where they'll land.
  const hasFiles = (event: DragEvent) => Array.from(event.dataTransfer.types).includes('Files');
  const dropProps = ready && !phone
    ? {
        onDragEnter: (event: DragEvent) => {
          if (!hasFiles(event)) return;
          event.preventDefault();
          dragDepth.current += 1;
          setDropping(true);
        },
        onDragOver: (event: DragEvent) => {
          if (!hasFiles(event)) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = 'copy';
        },
        onDragLeave: (event: DragEvent) => {
          if (!hasFiles(event)) return;
          dragDepth.current = Math.max(0, dragDepth.current - 1);
          if (!dragDepth.current) setDropping(false);
        },
        onDrop: (event: DragEvent) => {
          if (!hasFiles(event)) return;
          event.preventDefault();
          dragDepth.current = 0;
          setDropping(false);
          const files: File[] = [];
          let folders = 0;
          const items = Array.from(event.dataTransfer.items);
          Array.from(event.dataTransfer.files).forEach((file, index) => {
            if (items[index]?.webkitGetAsEntry?.()?.isDirectory) folders += 1;
            else files.push(file);
          });
          if (folders) notify.info(folders === 1 ? 'Folders can’t be uploaded. Compress it first, then upload the .zip.' : 'Folders can’t be uploaded. Compress them first, then upload the .zip files.');
          addFiles(files);
        },
      }
    : {};

  // A file dropped just outside the list shouldn't make the browser leave the portal to show it.
  useEffect(() => {
    const guard = (event: globalThis.DragEvent) => {
      if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
    };
    addEventListener('dragover', guard);
    addEventListener('drop', guard);
    return () => {
      removeEventListener('dragover', guard);
      removeEventListener('drop', guard);
    };
  }, []);

  const itemsLine = data
    ? data.truncated
      ? `Showing the first ${data.entries.length} items. Open this folder on the Mac to see the rest.`
      : count(data.entries.length, 'item')
    : null;

  const current = selected ? (entries?.find((item) => item.path === selected) ?? null) : null;
  const currentType = current ? fileType(current) : null;

  // Quick Look: on a Mac it follows the selection; on a phone it shows the file that was tapped.
  const lookingHere = looking && looking.folder === path ? looking.path : null;
  const lookItem = !lookingHere || !entries ? null : phone ? (entries.find((item) => item.path === lookingHere) ?? null) : current;
  useEffect(() => {
    if (looking && !lookItem && entries) setLooking(null);
  }, [looking, lookItem, entries]);

  const quickLook = useCallback(
    (item: FileItem) => {
      setLooking((now) => (now && now.folder === path && !phone ? null : { folder: path, path: item.path }));
      if (!phone) select(item.path);
    },
    [path, phone, select],
  );
  const closeLook = useCallback(() => setLooking(null), []);

  const rowFor = useCallback((at: string | null) => (at ? grid.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(at)}"]`) ?? null : null), []);
  const selectedNow = useRef(selected);
  useEffect(() => {
    selectedNow.current = selected;
  });
  const lookAnchor = useCallback(() => rowFor(selectedNow.current), [rowFor]);
  const lookReturn = useCallback(() => grid.current, []);
  const lookMove = useCallback(
    (delta: 1 | -1) => {
      if (!entries?.length) return;
      const at = entries.findIndex((item) => item.path === selectedNow.current);
      const next = entries[Math.max(0, Math.min(entries.length - 1, at + delta))];
      if (next) select(next.path);
    },
    [entries, select],
  );
  // The list scrolls with the selection while Quick Look has the keyboard.
  useEffect(() => {
    if (lookItem && !phone) rowFor(lookItem.path)?.scrollIntoView({ block: 'nearest' });
  }, [lookItem, phone, rowFor]);

  const actions = phone ? (
    <ActionMenu
      label="Add"
      trigger={
        <IconButton label="Add" disabled={!ready}>
          <Icon name="plus" size={22} weight={2.2} />
        </IconButton>
      }
      items={[
        { label: 'Upload Files', onSelect: startUpload },
        { label: 'New Folder', onSelect: startFolder },
      ]}
    />
  ) : (
    <>
      <ActionMenu
        label="Actions for the selected item"
        trigger={
          <IconButton label="Actions for the selected item" disabled={!ready || !current}>
            <Icon name="ellipsis" size={20} weight={2.6} />
          </IconButton>
        }
        items={[
          ...(currentType?.navigable ? [{ label: 'Open', onSelect: () => current && open(current) }] : []),
          { label: 'Quick Look', onSelect: () => current && quickLook(current), hint: 'Space' },
          { label: 'Download', onSelect: () => current && download(current), disabled: !currentType?.downloadable },
          { label: 'Move to Trash', onSelect: () => current && setTrashing(current), tone: 'destructive' as const, separated: true },
        ]}
      />
      <Button variant="plain" onClick={startFolder} disabled={!ready} icon={<Icon name="folder" size={17} weight={1.8} />}>
        New Folder
      </Button>
      <Button variant="plain" onClick={startUpload} disabled={!ready} icon={<Icon name="upload" size={17} weight={1.9} />}>
        Upload
      </Button>
    </>
  );

  const back = phone && parent ? { href: hrefFor(parent), label: crumbs(parent).at(-1)!.label } : { href: macPath(instance.id), label: instance.name };

  return (
    <Page title={phone && path !== '~' ? folderName : 'Files'} back={back} actions={actions} width="wide">
      <input
        ref={picker}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          addFiles(Array.from(event.currentTarget.files ?? []));
          event.currentTarget.value = '';
        }}
      />
      <div className={styles.page} {...dropProps}>
        <Uploads uploads={uploads.uploads} onCancel={uploads.cancel} onRetry={uploads.retry} onClear={uploads.clearFinished} />

        {offline ? (
          <NotRunning instance={instance} error={error!} power={power} onRetry={() => void listing.reload()} />
        ) : failed ? (
          <LoadError error={error} onRetry={() => void listing.reload()} onUp={parent ? goUp : null} onHome={() => navigate(base)} />
        ) : phone ? (
          entries && entries.length === 0 ? (
            <EmptyState icon="folder" title="This folder is empty">
              Tap <span className="visually-hidden">Add</span>
              <Icon name="plus" size={14} weight={2.4} className={styles.inlineGlyph} /> to upload files or make a new folder.
            </EmptyState>
          ) : (
            <PhoneFiles entries={entries} footer={itemsLine} onOpen={open} onDownload={download} onTrash={setTrashing} onQuickLook={quickLook} />
          )
        ) : (
          <>
            <PathBar trail={trail} hrefFor={hrefFor} />
            <FinderList
              label={`Files in ${folderName}`}
              entries={entries}
              sort={sort}
              onSort={(key) => {
                const next = nextSort(sort, key);
                setSort(next);
                saveSort(next);
              }}
              selected={selected}
              onSelect={select}
              onOpen={open}
              onUp={goUp}
              onDownload={download}
              onTrash={setTrashing}
              onNewFolder={startFolder}
              onUpload={startUpload}
              onQuickLook={quickLook}
              gridRef={grid}
              dropping={dropping}
              status={
                dropping ? (
                  <>
                    <Icon name="upload" size={13} weight={2.2} /> Drop to upload to “{folderName}”
                  </>
                ) : (
                  (itemsLine ?? (listing.loading ? 'Loading…' : null))
                )
              }
              empty={
                <EmptyState icon="folder" title="This folder is empty" action={<Button variant="tinted" onClick={startUpload}>Upload Files</Button>}>
                  Drag files here to upload them to the Mac, or make a new folder.
                </EmptyState>
              }
            />
          </>
        )}
      </div>

      {phone ? (
        <PhoneQuickLook instanceId={instance.id} macName={instance.name} running={on} item={lookItem} onClose={closeLook} onDownload={download} />
      ) : (
        <QuickLook
          instanceId={instance.id}
          macName={instance.name}
          running={on}
          item={lookItem}
          onClose={closeLook}
          onDownload={download}
          onMove={lookMove}
          anchor={lookAnchor}
          returnFocus={lookReturn}
        />
      )}
      <NewFolderSheet
        instanceId={instance.id}
        open={creating}
        onOpenChange={setCreating}
        folder={data ? { path: data.path, name: folderName, names: data.entries.map((item) => item.name) } : null}
        onCreated={(item) => {
          placeInListing(instance.id, path, item);
          select(item.path);
        }}
      />
      <Confirm
        open={!!trashing}
        onOpenChange={(next) => !next && setTrashing(null)}
        title={`Move “${trashing?.name ?? ''}” to the Trash?`}
        message={
          trashing?.kind === 'dir'
            ? `The folder and everything in it go to the Trash on ${instance.name}. You can put them back from there until it’s emptied.`
            : `It goes to the Trash on ${instance.name}. You can put it back from there until it’s emptied.`
        }
        action="Move to Trash"
        onConfirm={() => trashing && void trash(trashing)}
      />
    </Page>
  );
}

function PathBar({ trail, hrefFor }: { trail: ReturnType<typeof crumbs>; hrefFor: (path: string) => string }) {
  return (
    <nav className={styles.pathBar} aria-label="Folder path">
      <ol>
        {trail.map((crumb, index) => {
          const last = index === trail.length - 1;
          const glyph =
            crumb.path === '~' ? (
              <Icon name="house" size={14} weight={1.9} />
            ) : crumb.path === '/' ? (
              <Icon name="disk" size={14} weight={1.9} />
            ) : (
              <FileIcon item={{ name: crumb.label, kind: 'dir' }} size={15} />
            );
          return (
            <li key={crumb.path} className={last ? styles.here : undefined}>
              {index > 0 ? <Icon name="chevron-right" size={11} weight={2.4} className={styles.separator} /> : null}
              {last ? (
                <span className={styles.crumb} aria-current="location" title={crumb.label}>
                  {glyph}
                  <span className={styles.crumbLabel}>{crumb.label}</span>
                </span>
              ) : (
                <a className={styles.crumb} href={hrefFor(crumb.path)} onClick={(event) => onLinkClick(event, { direction: 'back' })} title={crumb.label}>
                  {glyph}
                  <span className={styles.crumbLabel}>{crumb.label}</span>
                </a>
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

function NotRunning({ instance, error, power, onRetry }: { instance: Instance; error: ApiError; power: ReturnType<typeof usePower>; onRetry: () => void }) {
  const info = stateInfo(instance.state);
  if (info.transitional) {
    return (
      // Familiar stays mounted between these two, so it wakes into thinking without a cut.
      <EmptyState familiar="thinking" title={`${instance.name} is starting`}>
        Its files show up here as soon as macOS is ready.
      </EmptyState>
    );
  }
  const primary = info.primary;
  return (
    <EmptyState
      familiar={primary && power.busy === primary.action ? 'waking' : 'sleeping'}
      title={`${instance.name} isn’t running`}
      action={
        primary ? (
          <Button variant="tinted" busy={power.busy === primary.action} onClick={() => power.run(primary.action)}>
            {primary.label}
          </Button>
        ) : (
          <Button variant="tinted" onClick={onRetry}>
            Try Again
          </Button>
        )
      }
    >
      {error.message}
    </EmptyState>
  );
}

function LoadError({ error, onRetry, onUp, onHome }: { error: ApiError; onRetry: () => void; onUp: (() => void) | null; onHome: () => void }) {
  if (error.code === 'forbidden') {
    return (
      <EmptyState icon="lock" title="Can’t open this folder" action={onUp ? <Button variant="tinted" onClick={onUp}>Go to Enclosing Folder</Button> : undefined}>
        {error.message} To open it from here, on the Mac open System Settings &gt; Privacy &amp; Security &gt; Full Disk Access and turn on dot-guest.
      </EmptyState>
    );
  }
  if (error.code === 'not_found' || error.code === 'invalid') {
    return (
      <EmptyState icon="folder" title="This folder isn’t there" action={<Button variant="tinted" onClick={onHome}>Open Home Folder</Button>}>
        {error.message} It may have been moved, renamed or put in the Trash.
      </EmptyState>
    );
  }
  return (
    <EmptyState icon="wifi" title="Couldn’t load files" action={<Button variant="tinted" onClick={onRetry}>Try Again</Button>}>
      {error.message}
    </EmptyState>
  );
}
