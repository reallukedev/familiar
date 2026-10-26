import { lazy, Suspense, useEffect } from 'react';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/Bits';
import { SkeletonBlock, SkeletonRows } from '../../components/Skeleton';
import { api } from '../../lib/api';
import { keys, peek, useQuery } from '../../lib/cache';
import { useLive } from '../../lib/live';
import { navigate } from '../../lib/router';
import type { MacSection } from '../../lib/routes';
import type { Instance } from '../../lib/types';
import { Page } from '../../shells/Page';
import { MAC_TITLES, rememberMac } from '../../shells/nav';
import { ActivityPage } from './Activity';
import { AppsPage } from './Apps';
import { ConnectPage } from './Connect';
import { FilesPage } from './Files';
import { Overview } from './Overview';
import { PermissionsPage } from './Permissions';
import { ReviewPage } from './Review';
import { SettingsPage } from './Settings';
import { SnapshotsPage } from './Snapshots';

const ScreenPage = lazy(() => import('./Screen'));

export type MacPageProps = { instance: Instance };

/** Loads one Mac, keeps its live events flowing, and shows the requested section. */
export function MacRoute({ id, section }: { id: string; section: MacSection }) {
  const fromList = peek<Instance[]>(keys.instances)?.find((item) => item.id === id);
  const query = useQuery<Instance>(keys.instance(id), () => api.instance(id), { staleMs: 10_000 });
  const instance = query.data ?? fromList;
  useLive(instance ? id : null);
  useEffect(() => rememberMac(id), [id]);

  if (!instance) {
    if (query.error?.code === 'not_found' || query.error?.code === 'forbidden') {
      return (
        <Page title="Mac Not Found" back={{ href: '/', label: 'Macs' }}>
          <EmptyState
            icon="display"
            title="This Mac isn’t here"
            action={
              <Button variant="tinted" onClick={() => navigate('/')}>
                Show My Macs
              </Button>
            }
          >
            It may have been deleted, or it belongs to someone else.
          </EmptyState>
        </Page>
      );
    }
    if (query.error) {
      return (
        <Page title={MAC_TITLES[section]} back={{ href: '/', label: 'Macs' }}>
          <EmptyState icon="wifi" title="Couldn’t load this Mac" action={<Button variant="tinted" onClick={() => void query.reload()}>Try Again</Button>}>
            {query.error.message}
          </EmptyState>
        </Page>
      );
    }
    return (
      <Page title={MAC_TITLES[section]} back={{ href: '/', label: 'Macs' }} width={section === 'overview' ? 'wide' : 'regular'}>
        <div role="status" aria-label="Loading Mac" style={{ display: 'grid', gap: 24, paddingTop: 8 }}>
          {section === 'overview' ? <SkeletonBlock height="min(56vw, 560px)" radius={18} /> : null}
          <SkeletonRows rows={4} />
        </div>
      </Page>
    );
  }

  switch (section) {
    case 'overview':
      return <Overview instance={instance} />;
    case 'screen':
      return (
        <Suspense fallback={<ScreenFallback instance={instance} />}>
          <ScreenPage instance={instance} />
        </Suspense>
      );
    case 'files':
      return <FilesPage instance={instance} />;
    case 'activity':
      return <ActivityPage instance={instance} />;
    case 'review':
      return <ReviewPage instance={instance} />;
    case 'apps':
      return <AppsPage instance={instance} />;
    case 'permissions':
      return <PermissionsPage instance={instance} />;
    case 'connect':
      return <ConnectPage instance={instance} />;
    case 'snapshots':
      return <SnapshotsPage instance={instance} />;
    case 'settings':
      return <SettingsPage instance={instance} />;
  }
}

function ScreenFallback({ instance }: MacPageProps) {
  return (
    <Page title={instance.name} back={{ href: `/macs/${encodeURIComponent(instance.id)}`, label: instance.name }} width="full">
      <div role="status" aria-label="Loading screen" />
    </Page>
  );
}
