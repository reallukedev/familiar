import { CSPProvider } from '@base-ui/react/csp-provider';
import { Toast } from '@base-ui/react/toast';
import { Tooltip } from '@base-ui/react/tooltip';
import { lazy, Suspense, useEffect, type ReactNode } from 'react';
import { Button } from './components/Button';
import { EmptyState } from './components/Bits';
import { Familiar } from './components/Familiar';
import { OfflineBanner } from './components/OfflineBanner';
import { ReauthSheet } from './components/ReauthSheet';
import { RecoveryKeySheet } from './components/RecoveryKeySheet';
import { SettingsWindow } from './components/SettingsWindow';
import { Spinner } from './components/Spinner';
import { Toaster } from './components/Toaster';
import { usePlatform } from './lib/platform';
import { navigate, useLocation } from './lib/router';
import { PUBLIC_ROUTES, safeNext, type Route } from './lib/routes';
import { totpGate, useSession } from './lib/session';
import { useStore } from './lib/store';
import { toastManager } from './lib/toast';
import { Account } from './pages/Account';
import { StoragePage } from './pages/Storage';
import { ActivityTab } from './pages/ActivityTab';
import { Consent } from './pages/Consent';
import { Invite } from './pages/Invite';
import { MacRoute } from './pages/mac/MacRoute';
import { Macs } from './pages/Macs';
import { NotFound } from './pages/NotFound';
import { Setup } from './pages/Setup';
import { SignIn } from './pages/SignIn';
import { TotpEnroll } from './pages/TotpEnroll';
import { DesktopShell } from './shells/DesktopShell';
import { PhoneShell } from './shells/PhoneShell';
import styles from './App.module.css';

const AdminRoute = lazy(() => import('./pages/admin/Admin'));
const BuilderScreen = lazy(() => import('./pages/admin/BuilderScreen'));

export function App() {
  return (
    <CSPProvider disableStyleElements>
      <Toast.Provider toastManager={toastManager} limit={3}>
        <Tooltip.Provider delay={600}>
          <Root />
          <ReauthSheet />
          <RecoveryKeySheet />
          <OfflineBanner />
          <Toaster />
        </Tooltip.Provider>
      </Toast.Provider>
    </CSPProvider>
  );
}

function Redirect({ to }: { to: string }) {
  useEffect(() => navigate(to, { replace: true }), [to]);
  return <Splash />;
}

function Splash() {
  return (
    <div className={styles.splash} role="status" aria-label="Loading Familiar">
      <Familiar state="thinking" size={64} />
    </div>
  );
}

function Root() {
  const session = useSession();
  const { route, path, search } = useLocation();
  const platform = usePlatform();
  const mustEnroll = useStore(totpGate);

  if (!session.data) {
    if (session.error) {
      return (
        <main className={styles.center}>
          <EmptyState familiar="offline" title="Can’t reach Familiar" action={<Button variant="filled" busy={session.loading} onClick={() => void session.reload()}>Try Again</Button>}>
            {session.error.message}
          </EmptyState>
        </main>
      );
    }
    return <Splash />;
  }

  const info = session.data;
  if (info.setup_required) return <Setup />;
  if (route.name === 'setup') return <Redirect to="/" />;

  if (!info.user) {
    if (route.name === 'invite') return <Invite code={route.code} />;
    if (route.name === 'signin') return <SignIn />;
    return <Redirect to={`/signin?next=${encodeURIComponent(path + search)}`} />;
  }
  if (route.name === 'signin') return <Redirect to={safeNext(new URLSearchParams(search).get('next'))} />;
  if (route.name === 'invite') return <Invite code={route.code} />;
  if (mustEnroll || (info.require_totp && !info.user.totp_enabled)) return <TotpEnroll />;
  if (route.name === 'consent') return <Consent />;

  const Shell = platform === 'phone' ? PhoneShell : DesktopShell;
  return (
    <Shell>
      <Suspense fallback={<PageFallback />}>
        <Screen route={route} />
      </Suspense>
      <SettingsWindow />
    </Shell>
  );
}

function PageFallback() {
  return (
    <div className={styles.fallback}>
      <Spinner size={22} label="Loading" />
    </div>
  );
}

function Screen({ route }: { route: Route }): ReactNode {
  switch (route.name) {
    case 'macs':
      return <Macs />;
    case 'mac':
      return <MacRoute key={route.id} id={route.id} section={route.section} />;
    case 'activity':
      return <ActivityTab />;
    case 'account':
      return <Account />;
    case 'storage':
      return <StoragePage />;
    case 'admin':
      return <AdminRoute section={route.section} />;
    case 'builder':
      return <BuilderScreen />;
    default:
      return PUBLIC_ROUTES.has(route.name) ? <Redirect to="/" /> : <NotFound />;
  }
}
