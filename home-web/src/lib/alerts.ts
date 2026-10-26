// Browser notification permission for approval alerts. Each device asks on its
// own, so the saved preference only matters once this browser has said yes.

export type AlertPermission = NotificationPermission | 'unsupported';

export function notificationPermission(): AlertPermission {
  return typeof Notification === 'undefined' ? 'unsupported' : Notification.permission;
}

export async function requestNotifications(): Promise<AlertPermission> {
  if (typeof Notification === 'undefined') return 'unsupported';
  try {
    return await Notification.requestPermission();
  } catch {
    return Notification.permission;
  }
}
