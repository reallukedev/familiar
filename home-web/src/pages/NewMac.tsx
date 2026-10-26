import { useEffect, useState, type FormEvent } from 'react';
import { CopyField } from '../components/Copy';
import { Fields, FormError, TextField } from '../components/Field';
import { Segmented } from '../components/Segmented';
import { Sheet } from '../components/Sheet';
import { ApiError, api } from '../lib/api';
import { putInstance } from '../lib/live';
import { navigate } from '../lib/router';
import { macPath } from '../lib/routes';
import { notify } from '../lib/toast';
import { useSession } from '../lib/session';
import styles from './NewMac.module.css';

export function installCommand(publicUrl: string) {
  return `curl -fsSL ${publicUrl.replace(/\/+$/, '')}/guest/install.sh | sh`;
}

const CPUS = [
  { value: '2', label: '2' },
  { value: '4', label: '4' },
  { value: '6', label: '6' },
  { value: '8', label: '8' },
] as const;
const MEMORY = [
  { value: '4096', label: '4 GB' },
  { value: '8192', label: '8 GB' },
  { value: '12288', label: '12 GB' },
  { value: '16384', label: '16 GB' },
] as const;
const DISK = [
  { value: '64', label: '64 GB' },
  { value: '128', label: '128 GB' },
  { value: '256', label: '256 GB' },
] as const;

export function NewMacSheet({ open, onOpenChange, suggestedName = 'My Mac' }: { open: boolean; onOpenChange: (open: boolean) => void; suggestedName?: string }) {
  const [name, setName] = useState(suggestedName);
  const [driver, setDriver] = useState<'qemu' | 'remote'>('qemu');
  const [cpus, setCpus] = useState('4');
  const [memory, setMemory] = useState('8192');
  const [disk, setDisk] = useState('128');
  const [host, setHost] = useState('');
  const [mode, setMode] = useState<'ask' | 'changes' | 'all'>('all');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pairing, setPairing] = useState<{ code: string; id: string } | null>(null);
  const publicUrl = useSession().data?.public_url ?? location.origin;

  useEffect(() => {
    if (!open) return;
    setName(suggestedName);
    setError(null);
    setPairing(null);
    setBusy(false);
  }, [open, suggestedName]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim()) {
      setError('Give the Mac a name.');
      return;
    }
    if (driver === 'remote' && !host.trim()) {
      setError('Enter the address of the Mac, for example mac-mini.local.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await api.createInstance(
        driver === 'qemu'
          ? { name: name.trim(), driver, cpus: Number(cpus), memory_mb: Number(memory), disk_gb: Number(disk), approval_mode: mode }
          : { name: name.trim(), driver, remote_host: host.trim(), approval_mode: mode },
      );
      putInstance(result.instance);
      if (result.pairing) {
        setPairing({ code: result.pairing, id: result.instance.id });
      } else {
        onOpenChange(false);
        notify.success(`${result.instance.name} is ready to start`);
        navigate(macPath(result.instance.id));
      }
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : 'Couldn’t create the Mac. Try again.');
    } finally {
      setBusy(false);
    }
  }

  if (pairing) {
    return (
      <Sheet open={open} onOpenChange={onOpenChange} title="Pair Your Mac" dismissLabel="Done">
        <div className={styles.stack}>
          <p className="t-body pretty">In Terminal on that Mac, run these two commands. The first installs the Familiar agent; the second pairs it with this account.</p>
          <ol className={styles.commands}>
            <li>
              <CopyField value={installCommand(publicUrl)} what="install command" prefix="$" />
            </li>
            <li>
              <CopyField value={`dot-guest pair '${pairing.code}'`} what="pairing command" prefix="$" />
            </li>
          </ol>
          <p className="t-footnote text-orange pretty">This code is shown once. Copy it now; if you lose it, delete this Mac and add it again.</p>
          <p className="t-footnote secondary pretty">The Mac shows up as ready as soon as the agent connects.</p>
        </div>
      </Sheet>
    );
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="New Mac" primary={{ label: 'Create Mac', form: 'new-mac', busy }}>
      <form id="new-mac" className={styles.stack} onSubmit={submit} noValidate>
        <Fields>
          <TextField label="Name" value={name} onValueChange={setName} maxLength={64} autoComplete="off" />
          <div className={styles.choice}>
            <span className="t-footnote secondary" id="where-label">
              Where it runs
            </span>
            <Segmented
              label="Where it runs"
              value={driver}
              onChange={setDriver}
              options={[
                { value: 'qemu', label: 'On this server' },
                { value: 'remote', label: 'A Mac you own' },
              ]}
            />
          </div>
          {driver === 'qemu' ? (
            <>
              <div className={styles.choice}>
                <span className="t-footnote secondary">Processor cores</span>
                <Segmented label="Processor cores" value={cpus} onChange={setCpus} options={CPUS} />
              </div>
              <div className={styles.choice}>
                <span className="t-footnote secondary">Memory</span>
                <Segmented label="Memory" value={memory} onChange={setMemory} options={MEMORY} />
              </div>
              <div className={styles.choice}>
                <span className="t-footnote secondary">Disk</span>
                <Segmented label="Disk" value={disk} onChange={setDisk} options={DISK} />
              </div>
              <p className="t-footnote secondary pretty">A new Mac starts from the server’s macOS image and stores only its own changes, so it’s ready in seconds. You can change cores and memory later.</p>
            </>
          ) : (
            <TextField
              label="Address"
              value={host}
              onValueChange={setHost}
              placeholder="mac-mini.local"
              autoCapitalize="none"
              spellCheck={false}
              description="A real Mac on your network (Tailscale or LAN) running dot-guest. It works with the same tools; sleep and snapshots stay with that Mac."
            />
          )}
          <div className={styles.choice}>
            <span className="t-footnote secondary">Your assistant asks</span>
            <Segmented
              label="Your assistant asks"
              value={mode}
              onChange={setMode}
              options={[
                { value: 'all', label: 'Never' },
                { value: 'changes', label: 'Before Changes' },
                { value: 'ask', label: 'Always' },
              ]}
            />
            <span className="t-footnote secondary pretty">
              {mode === 'all' ? 'It works on its own. Changes collect in Review for you to look over.' : mode === 'changes' ? 'It reads on its own and waits for you before changing or sending anything.' : 'It waits for you before everything it does.'}
            </span>
          </div>
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}
