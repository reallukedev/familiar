import { useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { EmptyState } from '../components/Bits';
import { Icon } from '../components/Icon';
import { ActionMenu } from '../components/Menu';
import { SkeletonRows } from '../components/Skeleton';
import { api } from '../lib/api';
import { keys, useQuery } from '../lib/cache';
import { useLive } from '../lib/live';
import { usePlatform } from '../lib/platform';
import { navigate } from '../lib/router';
import { macPath } from '../lib/routes';
import type { Instance } from '../lib/types';
import { Page } from '../shells/Page';
import { recallMac, rememberMac } from '../shells/nav';
import { ActivityPage } from './mac/Activity';
import styles from './ActivityTab.module.css';

/** The phone's Activity tab: the last Mac you looked at, with a switcher. */
export function ActivityTab() {
  const desktop = usePlatform() === 'desktop';
  const instances = useQuery<Instance[]>(keys.instances, api.instances);
  const list = instances.data ?? [];
  const [remembered, setRemembered] = useState(recallMac);
  const instance = list.find((item) => item.id === remembered) ?? list[0] ?? null;
  useLive(instance?.id ?? null);

  useEffect(() => {
    if (desktop && instance) navigate(macPath(instance.id, 'activity'), { replace: true });
  }, [desktop, instance]);

  if (!instances.data) {
    return (
      <Page title="Activity">
        <div className={styles.loading}>
          <SkeletonRows rows={5} label="Loading activity" />
        </div>
      </Page>
    );
  }
  if (!instance) {
    return (
      <Page title="Activity">
        <EmptyState familiar="idle" title="No Macs yet" action={<Button variant="tinted" onClick={() => navigate('/')}>Create Your Mac</Button>}>
          Activity shows what your assistant did on each of your Macs.
        </EmptyState>
      </Page>
    );
  }
  const switcher =
    list.length > 1 ? (
      <ActionMenu
        label="Choose a Mac"
        trigger={
          <button type="button" className={styles.switcher}>
            <span className={styles.switcherName}>{instance.name}</span>
            <Icon name="chevron-down" size={14} weight={2.4} />
          </button>
        }
        items={list.map((item) => ({ label: item.name, onSelect: () => { rememberMac(item.id); setRemembered(item.id); } }))}
      />
    ) : null;
  return <ActivityPage key={instance.id} instance={instance} title="Activity" back={null} actions={switcher} />;
}
