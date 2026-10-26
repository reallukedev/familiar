import { ApiError, api } from '../../lib/api';
import { transport } from '../../lib/transport';
import type { FileItem } from '../../lib/types';

function save(href: string, name: string) {
  const link = document.createElement('a');
  link.href = href;
  link.download = name;
  link.rel = 'noopener';
  document.body.append(link);
  link.click();
  link.remove();
}

/**
 * Download one file from the Mac. The browser streams it straight to disk from
 * the download URL (files run to 2 GB). The demo backend only answers through
 * the transport, so there the bytes come back as a Blob first.
 */
export async function downloadFile(instanceId: string, item: FileItem) {
  const url = api.downloadUrl(instanceId, item.path);
  if (!document.documentElement.dataset.demo) {
    save(url, item.name);
    return;
  }
  const response = await transport.fetch(url, { method: 'GET', credentials: 'same-origin', cache: 'no-store' });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: { message?: string; code?: string } };
    throw new ApiError(body.error?.code ?? 'internal', body.error?.message ?? 'Couldn’t download the file. Try again.', response.status);
  }
  const blob = URL.createObjectURL(await response.blob());
  save(blob, item.name);
  setTimeout(() => URL.revokeObjectURL(blob), 30_000);
}
