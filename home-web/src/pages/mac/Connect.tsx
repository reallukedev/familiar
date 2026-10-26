import { useState, type FormEvent } from 'react';
import { EmptyState } from '../../components/Bits';
import { Button } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { CopyField } from '../../components/Copy';
import { Fields, FormError, TextField } from '../../components/Field';
import { Icon } from '../../components/Icon';
import { CellBody, CellText, Row, Section } from '../../components/List';
import { Guide } from '../../components/connect/Guide';
import { useCreateToken } from '../../components/connect/useCreateToken';
import { Picker } from '../../components/Picker';
import { Segmented } from '../../components/Segmented';
import { Sheet } from '../../components/Sheet';
import { SkeletonRows } from '../../components/Skeleton';
import { ApiError, api } from '../../lib/api';
import { keys, updateData, useQuery } from '../../lib/cache';
import { CLIENTS, type ClientId, claudeCodeCommand, findClient, serverId, uniqueTokenName } from '../../lib/clients';
import { day, whenInline } from '../../lib/format';
import { macPath } from '../../lib/routes';
import { notify } from '../../lib/toast';
import type { AccessToken, Connections, Grant } from '../../lib/types';
import { Page } from '../../shells/Page';
import type { MacPageProps } from './MacRoute';
import styles from './Connect.module.css';
import { withUndo } from '../../lib/undo';

const CLIENT_OPTIONS = CLIENTS.map((client) => ({ value: client.id, label: client.name }));

export function ConnectPage({ instance }: MacPageProps) {
  const connections = useQuery<Connections>(keys.connections(instance.id), () => api.connections(instance.id));
  const mcpUrl = connections.data?.mcp_url ?? instance.mcp_url;
  const [clientId, setClientId] = useState<ClientId>('chatgpt');
  const client = findClient(clientId) ?? CLIENTS[0]!;
  // A token made in a guide, kept only while this page is open: Familiar never shows it again.
  const [secrets, setSecrets] = useState<Partial<Record<ClientId, string>>>({});
  const tokens = useCreateToken(instance.id);
  const server = serverId(instance.name);
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<{ kind: 'token'; item: AccessToken } | { kind: 'grant'; item: Grant } | null>(null);
  const now = Date.now();

  async function revoke() {
    if (!revoking) return;
    try {
      if (revoking.kind === 'token') {
        await api.revokeToken(instance.id, revoking.item.id);
        updateData<Connections>(keys.connections(instance.id), (data) => ({ ...data, tokens: data.tokens.filter((token) => token.id !== revoking.item.id) }));
        notify.success(`Revoked ${revoking.item.name}`);
      } else {
        await api.revokeGrant(instance.id, revoking.item.id);
        updateData<Connections>(keys.connections(instance.id), (data) => ({ ...data, grants: data.grants.filter((grant) => grant.id !== revoking.item.id) }));
        notify.success(`Disconnected ${revoking.item.client_name}`);
      }
    } catch (error) {
      notify.error(error);
    }
  }

  async function createGuideToken() {
    const name = uniqueTokenName(client.tokenName, connections.data?.tokens ?? []);
    const created = await tokens.create(name, 90);
    if (created) setSecrets((all) => ({ ...all, [client.id]: created.secret }));
  }

  return (
    <Page title="Connections" back={{ href: macPath(instance.id), label: instance.name }}>
      <Section title="Server address" footer="Each connection reaches only this Mac.">
        <CellBody>
          <CopyField value={mcpUrl} what="server address" />
        </CellBody>
      </Section>

      <Section title="Connect an assistant">
        <Row
          title="Assistant"
          trailing={
            <span className={styles.picker}>
              <Picker
                label="Assistant"
                value={clientId}
                onChange={(next) => {
                  tokens.clearError();
                  setClientId(next);
                }}
                options={CLIENT_OPTIONS}
              />
            </span>
          }
        />
        <CellBody>
          <Guide
            key={client.id}
            client={client}
            context={{ url: mcpUrl, mac: instance.name, server, token: secrets[client.id] ?? null }}
            onCreateToken={() => void createGuideToken()}
            creating={tokens.busy}
            error={tokens.error}
          />
        </CellBody>
      </Section>

      <Section title="Connected apps">
        {connections.data ? (
          connections.data.grants.length ? (
            connections.data.grants.map((grant) => (
              <Row
                key={grant.id}
                title={grant.client_name}
                subtitle={grant.last_used_at ? `Last used ${whenInline(grant.last_used_at, now)}` : 'Not used yet'}
                trailing={
                  <Button variant="plain" size="small" onClick={() => setRevoking({ kind: 'grant', item: grant })}>
                    Disconnect
                  </Button>
                }
              />
            ))
          ) : (
            <CellText>No assistants are connected to this Mac yet.</CellText>
          )
        ) : connections.error ? (
          <CellText>{connections.error.message}</CellText>
        ) : (
          <SkeletonRows rows={2} />
        )}
      </Section>

      <Section
        title="Personal access tokens"
        accessory={
          <Button variant="plain" size="small" icon={<Icon name="plus" size={14} weight={2.4} />} onClick={() => setCreating(true)}>
            Create Token
          </Button>
        }
        footer="For Claude Code and scripts. Works only for this Mac."
      >
        {connections.data ? (
          connections.data.tokens.length ? (
            connections.data.tokens.map((token) => (
              <Row
                key={token.id}
                title={token.name}
                subtitle={
                  <>
                    <code>{token.prefix}…</code> · {token.last_used_at ? `Used ${whenInline(token.last_used_at, now)}` : 'Never used'}
                    {token.expires_at ? ` · Expires ${day(token.expires_at)}` : ''}
                  </>
                }
                trailing={
                  <Button variant="plain" size="small" onClick={() => setRevoking({ kind: 'token', item: token })}>
                    Revoke
                  </Button>
                }
              />
            ))
          ) : (
            <CellText>No tokens yet.</CellText>
          )
        ) : connections.error ? null : (
          <SkeletonRows rows={1} />
        )}
      </Section>

      {connections.error && !connections.data ? (
        <EmptyState icon="wifi" title="Couldn’t load connections" action={<Button variant="tinted" onClick={() => void connections.reload()}>Try Again</Button>}>
          {connections.error.message}
        </EmptyState>
      ) : null}

      <CreateTokenSheet instanceId={instance.id} mcpUrl={mcpUrl} server={server} open={creating} onOpenChange={setCreating} />
      <Confirm
        open={!!revoking}
        onOpenChange={(open) => !open && setRevoking(null)}
        title={revoking?.kind === 'grant' ? `Disconnect ${revoking.item.client_name}?` : `Revoke ${revoking?.item && 'name' in revoking.item ? revoking.item.name : 'this token'}?`}
        message={revoking?.kind === 'grant' ? 'It stops working on this Mac right away, along with every token it was given.' : 'Anything using this token stops working right away.'}
        action={revoking?.kind === 'grant' ? 'Disconnect' : 'Revoke'}
        onConfirm={() => withUndo(revoking?.kind === 'grant' ? 'Disconnecting…' : 'Revoking token…', revoke)}
      />
    </Page>
  );
}

const EXPIRY = [
  { value: '30', label: '30 days' },
  { value: '90', label: '90 days' },
  { value: '365', label: '1 year' },
  { value: 'never', label: 'Never' },
] as const;

function CreateTokenSheet({ instanceId, mcpUrl, server, open, onOpenChange }: { instanceId: string; mcpUrl: string; server: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [name, setName] = useState('Claude Code');
  const [expiry, setExpiry] = useState<(typeof EXPIRY)[number]['value']>('90');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<{ token: AccessToken; secret: string } | null>(null);

  function close(next: boolean) {
    onOpenChange(next);
    if (!next) {
      setTimeout(() => {
        setCreated(null);
        setError(null);
        setName('Claude Code');
      }, 400);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim()) {
      setError('Name the token after where you’ll use it.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await api.createToken(instanceId, { name: name.trim(), ...(expiry === 'never' ? {} : { expires_in_days: Number(expiry) }) });
      updateData<Connections>(keys.connections(instanceId), (data) => ({ ...data, tokens: [result.token, ...data.tokens] }));
      setCreated(result);
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.silent)) setError(cause instanceof ApiError ? cause.message : 'Couldn’t create the token. Try again.');
    } finally {
      setBusy(false);
    }
  }

  if (created) {
    return (
      <Sheet open={open} onOpenChange={close} title="Token Created" primary={{ label: 'I Saved It', onClick: () => close(false) }} dismissLabel="Close">
        <div className={styles.created}>
          <p className="t-body pretty">Copy it now. Familiar won’t show it again.</p>
          <CopyField value={created.secret} what="token" />
          <p className="t-footnote secondary">To add this Mac to Claude Code, run:</p>
          <CopyField value={claudeCodeCommand(server, mcpUrl, created.secret)} what="command" prefix="$" />
        </div>
      </Sheet>
    );
  }

  return (
    <Sheet open={open} onOpenChange={close} title="New Token" primary={{ label: 'Create Token', form: 'new-token', busy }}>
      <form id="new-token" className={styles.created} onSubmit={submit} noValidate>
        <Fields>
          <TextField label="Name" value={name} onValueChange={setName} maxLength={64} description="Where you’ll use it, so you know what to revoke later." />
          <div className={styles.choice}>
            <span className="t-footnote secondary">Expires after</span>
            <Segmented label="Expires after" value={expiry} onChange={setExpiry} options={EXPIRY} />
          </div>
        </Fields>
        <FormError>{error}</FormError>
      </form>
    </Sheet>
  );
}
