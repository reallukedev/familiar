import { Button } from '../components/Button';
import { TotpSetup } from '../components/TotpSetup';
import { finishEnrollment, signOut, useSession } from '../lib/session';
import { notify } from '../lib/toast';
import { AuthLayout } from './AuthLayout';
import styles from './Forms.module.css';

/**
 * Required two-factor enrollment. It can't be dismissed: the server refuses
 * everything else until it's done. Afterwards the person lands where they were going.
 */
export function TotpEnroll() {
  const { data: session } = useSession();
  return (
    <AuthLayout title="Turn on two‑factor authentication" footer={<>Signed in as {session?.user?.username}. <Button variant="plain" size="small" onClick={() => void signOut()}>Sign Out</Button></>}>
      <p className={`t-body secondary ${styles.lead}`}>Familiar controls real Macs, so every account needs a code from an authenticator app as well as a password.</p>
      <TotpSetup
        confirmLabel="Turn On and Continue"
        onEnrolled={(user) => {
          notify.success('Two-factor authentication is on');
          void finishEnrollment(user);
        }}
      />
    </AuthLayout>
  );
}
