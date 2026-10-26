import { useEffect, useState, type FormEvent } from 'react';
import { ApiError, api } from '../../lib/api';
import { joinPath, nameError, uniqueName } from '../../lib/files';
import type { FileItem } from '../../lib/types';
import { Fields, FormError, TextField } from '../Field';
import { Sheet } from '../Sheet';
import styles from './NewFolderSheet.module.css';

type Props = {
  instanceId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The folder's absolute path on the Mac, and what's in it now. */
  folder: { path: string; name: string; names: string[] } | null;
  onCreated: (item: FileItem) => void;
};

/** Name a new folder, starting from Finder's "untitled folder", numbered if taken. */
export function NewFolderSheet({ instanceId, open, onOpenChange, folder, onCreated }: Props) {
  const [name, setName] = useState('');
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !folder) return;
    setName(uniqueName(folder.names, 'untitled folder'));
    setTouched(false);
    setError(null);
    setServerError(null);
    // Reset only when the sheet opens; the listing can refresh underneath it.
  }, [open]);

  const problem = folder ? nameError(name, folder.names) : null;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!folder) return;
    setTouched(true);
    if (problem) return;
    setBusy(true);
    setError(null);
    setServerError(null);
    try {
      const item = await api.createFolder(instanceId, joinPath(folder.path, name.trim()));
      onCreated(item);
      onOpenChange(false);
    } catch (cause) {
      if (cause instanceof ApiError && (cause.field === 'path' || cause.field === 'name' || cause.code === 'conflict')) setServerError(cause.message);
      else setError(cause instanceof ApiError ? cause.message : 'Couldn’t make the folder. Try again.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={onOpenChange}
      title="New Folder"
      description={folder ? `In “${folder.name}”` : undefined}
      primary={{ label: 'Create', form: 'new-folder', busy }}
    >
      <form id="new-folder" className={styles.form} onSubmit={submit} noValidate>
        <Fields>
          <TextField
            label="Name"
            value={name}
            onValueChange={(next) => {
              setName(next);
              setTouched(true);
              setServerError(null);
            }}
            error={serverError ?? (touched ? problem : null)}
            autoFocus
            onFocus={(event) => event.currentTarget.select()}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            enterKeyHint="done"
          />
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}
