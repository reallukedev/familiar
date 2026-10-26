import { Radio } from '@base-ui/react/radio';
import { RadioGroup } from '@base-ui/react/radio-group';
import { useEffect, useState } from 'react';
import { StatusLabel } from '../components/Bits';
import { Button } from '../components/Button';
import { FormError } from '../components/Field';
import { Icon } from '../components/Icon';
import { MacScreen } from '../components/MacScreen';
import { SkeletonBlock } from '../components/Skeleton';
import { ApiError, api } from '../lib/api';
import { keys, useQuery } from '../lib/cache';
import { safeRedirect } from '../lib/routes';
import { useSearchParam } from '../lib/router';
import { stateInfo } from '../lib/status';
import type { Instance } from '../lib/types';
import { NewMacSheet } from './NewMac';
import { AuthLayout } from './AuthLayout';
import styles from './Consent.module.css';

/** OAuth consent: an assistant asks to use one of your Macs; you pick which, or say no. */
export function Consent() {
  const requestId = useSearchParam('request');
  // Both requests start together: neither waits on the other.
  const request = useQuery(requestId ? `oauth:${requestId}` : null, () => api.oauthRequest(requestId!), { staleMs: 60_000 });
  const instances = useQuery<Instance[]>(keys.instances, api.instances);
  const [picked, setPicked] = useState<string | null>(null);
  const [busy, setBusy] = useState<'allow' | 'deny' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [leaving, setLeaving] = useState<string | null>(null);

  useEffect(() => {
    if (!picked && instances.data?.length) setPicked(instances.data[0]!.id);
  }, [instances.data, picked]);

  async function answer(approve: boolean) {
    if (!requestId) return;
    if (approve && !picked) {
      setError('Choose a Mac for the assistant to use.');
      return;
    }
    setBusy(approve ? 'allow' : 'deny');
    setError(null);
    try {
      const { redirect } = await api.answerOAuth(requestId, { instance_id: picked ?? '', approve });
      const target = safeRedirect(redirect);
      if (!target) throw new ApiError('bad_response', 'The assistant gave an address Familiar won’t open. Start connecting again from the assistant.');
      setLeaving(new URL(target).host);
      location.assign(target);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : 'Couldn’t send your answer. Try again.');
      setBusy(null);
    }
  }

  if (!requestId || request.error) {
    const expired = request.error?.code === 'not_found';
    return (
      <AuthLayout title={expired || !requestId ? 'This request has expired' : 'Couldn’t load the request'}>
        <p className={`t-body secondary ${styles.center}`}>{expired || !requestId ? 'Go back to your assistant and connect again. Requests last a few minutes.' : request.error?.message}</p>
      </AuthLayout>
    );
  }
  if (!request.data || !instances.data) {
    return (
      <AuthLayout title="Connect an assistant" wide>
        <div className={styles.loading} role="status" aria-label="Loading request">
          <SkeletonBlock height={60} />
          <SkeletonBlock height={120} />
        </div>
      </AuthLayout>
    );
  }
  if (leaving) {
    return (
      <AuthLayout title={busy === 'allow' ? 'Connected' : 'Not connected'}>
        <p className={`t-body secondary ${styles.center}`}>Returning to {leaving}…</p>
      </AuthLayout>
    );
  }

  const client = request.data.client_name;
  const list = instances.data;
  return (
    <AuthLayout title={`Allow ${client} to use a Mac?`} wide footer={`You’ll go back to ${request.data.redirect_host} after you choose. You can disconnect ${client} any time in the Mac’s Connections.`}>
      <p className={`t-body secondary pretty ${styles.center}`}>
        {client} will be able to see the screen, click, type and use apps on the Mac you choose, within the permissions you’ve set for it. It can’t reach your other Macs.
      </p>

      {list.length ? (
        <RadioGroup value={picked} onValueChange={(value) => setPicked(value as string)} aria-label="Mac to connect" className={styles.macs}>
          {list.map((instance) => {
            const info = stateInfo(instance.state);
            return (
              <label key={instance.id} className={styles.mac} onClick={() => setPicked(instance.id)}>
                <span className={styles.thumb}>
                  <MacScreen instance={instance} size="tiny" />
                </span>
                <span className={styles.macText}>
                  <span className="t-headline">{instance.name}</span>
                  <StatusLabel tone={info.tone}>{info.label}</StatusLabel>
                </span>
                <Radio.Root value={instance.id} className={styles.radio}>
                  <Radio.Indicator className={styles.dot} />
                </Radio.Root>
              </label>
            );
          })}
        </RadioGroup>
      ) : (
        <div className={styles.none}>
          <p className="t-subhead secondary">You don’t have a Mac yet. Create one, then allow {client} to use it.</p>
          <Button variant="tinted" icon={<Icon name="plus" size={14} weight={2.4} />} onClick={() => setCreating(true)}>
            Create a Mac
          </Button>
        </div>
      )}

      <FormError>{error}</FormError>
      <div className={styles.actions}>
        <Button variant="gray" size="large" onClick={() => void answer(false)} busy={busy === 'deny'} disabled={!!busy}>
          Don’t Allow
        </Button>
        <Button variant="filled" size="large" onClick={() => void answer(true)} busy={busy === 'allow'} disabled={!!busy || !picked}>
          Allow
        </Button>
      </div>
      <NewMacSheet open={creating} onOpenChange={setCreating} />
    </AuthLayout>
  );
}
