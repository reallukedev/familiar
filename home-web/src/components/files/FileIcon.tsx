import { fileType } from '../../lib/files';
import type { FileItem } from '../../lib/types';
import styles from './FileIcon.module.css';

// Finder-style icons drawn locally: a blue folder, a white page with a folded
// corner and a small mark for the kind, an app tile, an alias arrow.

const PAGE = 'M6 2.75h8.25L19 7.5v13.25a.75.75 0 0 1-.75.75H6a.75.75 0 0 1-.75-.75V3.5A.75.75 0 0 1 6 2.75Z';
const FOLD = 'M14.25 2.75V6.75a.75.75 0 0 0 .75.75H19';

const MARKS = {
  doc: <path className={styles.lines} d="M8.5 11h7M8.5 13.75h7M8.5 16.5h4.5" />,
  text: <path className={`${styles.lines} ${styles.muted}`} d="M8.5 11h7M8.5 13.75h7M8.5 16.5h5.5" />,
  code: <path className={`${styles.lines} ${styles.orange}`} d="M10 11.25 8 13.75l2 2.5M14.25 11.25l2 2.5-2 2.5" />,
  image: (
    <>
      <circle className={styles.sun} cx="10" cy="11" r="1.3" />
      <path className={styles.hill} d="M7.75 18l3.25-4 2 2.25 1.5-1.5 2.75 3.25Z" />
    </>
  ),
  video: <path className={styles.play} d="M10.25 11v6.25l5.25-3.13Z" />,
  audio: <path className={`${styles.lines} ${styles.red}`} d="M11.75 17.25V10.5l4-1v6.25M11.75 17.25a1.5 1.5 0 1 1-1.5-1.5 1.5 1.5 0 0 1 1.5 1.5ZM15.75 15.75a1.5 1.5 0 1 1-1.5-1.5 1.5 1.5 0 0 1 1.5 1.5Z" />,
  archive: (
    <>
      <path className={`${styles.lines} ${styles.muted}`} d="M11.5 3.5h1M12.5 5h1M11.5 6.5h1M12.5 8h1M11.5 9.5h1" />
      <rect className={styles.clasp} x="10.75" y="11.25" width="3.5" height="4.5" rx="0.8" />
    </>
  ),
} as const;

export function FileIcon({ item, size = 16 }: { item: Pick<FileItem, 'name' | 'kind'>; size?: number }) {
  const { glyph } = fileType(item);
  return (
    <svg className={styles.icon} width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      {glyph === 'folder' ? (
        <>
          <path className={styles.folderBack} d="M2.5 6.25A1.75 1.75 0 0 1 4.25 4.5h4.6c.46 0 .9.18 1.23.5l1.42 1.37h8.25a1.75 1.75 0 0 1 1.75 1.75v1.13H2.5Z" />
          <path className={styles.folderFront} d="M2.5 8.75A1.25 1.25 0 0 1 3.75 7.5h16.5a1.25 1.25 0 0 1 1.25 1.25v9.5A1.75 1.75 0 0 1 19.75 20H4.25a1.75 1.75 0 0 1-1.75-1.75Z" />
        </>
      ) : glyph === 'app' ? (
        <>
          <rect className={styles.app} x="3" y="3" width="18" height="18" rx="4.5" />
          <path className={styles.appMark} d="M9 16.25l3-8 3 8M10.1 13.5h3.8" />
        </>
      ) : (
        <>
          <path className={styles.page} d={PAGE} />
          <path className={styles.fold} d={FOLD} />
          {glyph === 'alias' ? (
            <>
              <path className={`${styles.lines} ${styles.muted}`} d="M8.5 11h7M8.5 13.75h4" />
              <rect className={styles.badge} x="5.75" y="15" width="6" height="6" rx="1.2" />
              <path className={styles.badgeArrow} d="M7.5 19.25l2.5-2.5M8 16.75h2v2" />
            </>
          ) : (
            MARKS[glyph]
          )}
        </>
      )}
    </svg>
  );
}
