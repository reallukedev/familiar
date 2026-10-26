import { Button } from '../components/Button';
import { EmptyState } from '../components/Bits';
import { navigate } from '../lib/router';
import { Page } from '../shells/Page';

export function NotFound() {
  return (
    <Page title="Not Found">
      <EmptyState familiar="sad" title="There’s nothing at this address" action={<Button variant="tinted" onClick={() => navigate('/')}>Show My Macs</Button>}>
        The link may be old, or the page moved.
      </EmptyState>
    </Page>
  );
}
