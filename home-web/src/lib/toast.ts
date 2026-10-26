import { Toast } from '@base-ui/react/toast';
import { ApiError } from './api';

export const toastManager = Toast.createToastManager();

export const notify = {
  success(title: string) {
    toastManager.add({ title, type: 'success', timeout: 2600 });
  },
  info(title: string) {
    toastManager.add({ title, type: 'info', timeout: 3200 });
  },
  /** Shows the server's own sentence; silent errors (a cancelled confirm) show nothing. */
  error(error: unknown, fallback = 'Something went wrong. Try again.') {
    if (error instanceof ApiError && (error.silent || error.code === 'network')) return; // The offline banner covers network failures.
    if (error instanceof DOMException && error.name === 'AbortError') return;
    const title = error instanceof ApiError ? error.message : fallback;
    toastManager.add({ title, type: 'error', timeout: 6000, priority: 'high' });
  },
};
