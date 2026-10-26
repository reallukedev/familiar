import { Button } from '../../components/Button';
import { EmptyState } from '../../components/Bits';
import { api } from '../../lib/api';
import { keys, useQuery } from '../../lib/cache';
import { navigate } from '../../lib/router';
import { useSession } from '../../lib/session';
import type { ImageStatus, Instance } from '../../lib/types';
import { Page } from '../../shells/Page';
import ScreenPage from '../mac/Screen';

/**
 * The image builder's screen. The builder never appears among the admin's
 * Macs, so it gets a stand-in Instance carrying just what the screen needs.
 */
export default function BuilderScreen() {
  const { data: session } = useSession();
  const image = useQuery<ImageStatus>(keys.images, api.images);
  const builder = image.data?.builder;
  if (session?.user?.role !== 'admin' || (image.data && !builder)) {
    return (
      <Page title="Installer" back={{ href: '/admin/image', label: 'macOS Image' }}>
        <EmptyState icon="disc" title="No image is being built" action={<Button variant="tinted" onClick={() => navigate('/admin/image')}>Show macOS Image</Button>} />
      </Page>
    );
  }
  if (!builder) return <Page title="Installer" back={{ href: '/admin/image', label: 'macOS Image' }}><div role="status" aria-label="Loading" /></Page>;
  const instance: Instance = {
    id: builder.instance_id,
    name: 'macOS Installer',
    driver: 'qemu',
    state: builder.phase === 'failed' ? 'error' : 'ready',
    state_detail: builder.detail,
    paused: false,
    cpus: 4,
    memory_mb: 8192,
    disk_gb: 128,
    sleep_after_minutes: 0,
    hibernate_after_minutes: 0,
    remote_host: null,
    created_at: new Date().toISOString(),
    last_active_at: null,
    booted_at: null,
    guest: null,
    mcp_url: '',
    counts: { calls_24h: 0, errors_24h: 0, pending_approvals: 0 },
  };
  return <ScreenPage instance={instance} back={{ href: '/admin/image', label: 'macOS Image' }} />;
}
