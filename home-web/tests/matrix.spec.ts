import { expect, test } from '@playwright/test';
import { open, shot } from './helpers';

// The state matrix, captured in both shells for review. Files land in
// test-results/screens/<project>-<name>.png.
test.use({ contextOptions: { reducedMotion: 'reduce' } });

const STATES: [string, string, string][] = [
  ['firstrun-setup', '/setup?token=demo-setup', 'firstrun'],
  ['signed-out', '/signin', 'signedout'],
  ['invite', '/invite/demo-join', 'signedout'],
  ['reset-invite', '/invite/demo-reset', 'signedout'],
  ['enroll-required', '/', 'enroll'],
  ['macs-zero', '/', 'empty'],
  ['macs-one', '/', 'one'],
  ['macs-many', '/', 'many'],
  ['macs-long-names', '/', 'longnames'],
  ['mac-ready-1-approval', '/macs/studio', 'one'],
  ['mac-3-approvals', '/macs/studio', 'approvals'],
  ['mac-degraded-missing-permissions', '/macs/studio', 'errors'],
  ['mac-error', '/macs/render', 'errors'],
  ['mac-sleeping', '/macs/work', 'many'],
  ['mac-hibernated', '/macs/photos', 'many'],
  ['mac-booting', '/macs/build', 'many'],
  ['mac-starting', '/macs/test', 'many'],
  ['mac-stopped', '/macs/old', 'many'],
  ['mac-stopping', '/macs/scratch', 'many'],
  ['mac-paused', '/macs/writer', 'many'],
  ['mac-remote-paused', '/macs/mini', 'many'],
  ['mac-never-connected', '/macs/fresh', 'noguest'],
  ['activity-many', '/macs/studio/activity', 'one'],
  ['activity-errors', '/macs/studio/activity', 'errors'],
  ['activity-empty', '/macs/fresh/activity', 'noguest'],
  ['apps-many', '/macs/studio/apps', 'one'],
  ['apps-long-names', '/macs/studio/apps', 'longnames'],
  ['apps-empty', '/macs/fresh/apps', 'noguest'],
  ['permissions', '/macs/studio/permissions', 'one'],
  ['connections', '/macs/studio/connect', 'one'],
  ['snapshots', '/macs/studio/snapshots', 'one'],
  ['settings', '/macs/studio/settings', 'one'],
  ['account', '/account', 'one'],
  ['admin-overview', '/admin', 'admin'],
  ['admin-failing-checks', '/admin', 'errors'],
  ['admin-people', '/admin/users', 'admin'],
  ['admin-invites', '/admin/invites', 'admin'],
  ['admin-all-macs', '/admin/macs', 'admin'],
  ['admin-image', '/admin/image', 'admin'],
  ['admin-image-failed', '/admin/image', 'errors'],
  ['admin-policies', '/admin/settings', 'admin'],
  ['updates-up-to-date', '/admin/updates', 'updates'],
  ['updates-nightly', '/admin/updates', 'updates-nightly'],
  ['mac-settings-agent-update', '/macs/studio/settings', 'updates-available'],
  ['member-home', '/', 'member'],
  ['consent', '/connect?request=req-chatgpt', 'many'],
  ['not-found', '/nowhere', 'one'],
];

for (const [name, path, scenario] of STATES) {
  test(`state: ${name}`, async ({ page }, info) => {
    await open(page, path, scenario);
    await page.waitForTimeout(900);
    await shot(page, info, name, true);
  });
}

test('dark appearance', async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  for (const [name, path, scenario] of [['dark-macs', '/', 'many'], ['dark-mac', '/macs/studio', 'one'], ['dark-updates', '/admin/updates', 'updates-available'], ['dark-apps', '/macs/studio/apps', 'one'], ['dark-signin', '/signin', 'signedout']]) {
    await open(page, path!, scenario);
    await page.waitForTimeout(900);
    await shot(page, info, name!, true);
  }
});

test('320 points wide', async ({ page }, info) => {
  await page.setViewportSize({ width: 320, height: 640 });
  for (const [name, path, scenario] of [['320-macs', '/', 'approvals'], ['320-mac', '/macs/studio', 'errors'], ['320-connect', '/macs/studio/connect', 'one'], ['320-updates', '/admin/updates', 'updates-nightly'], ['320-agent', '/macs/work/settings', 'updates-available'], ['320-signin', '/signin', 'signedout']]) {
    await open(page, path!, scenario);
    await page.waitForTimeout(700);
    await shot(page, info, name!, true);
  }
});

test('sheets and menus', async ({ page }, info) => {
  await open(page, '/macs/studio/apps');
  await page.getByRole('button', { name: /Install App|Add apps/ }).first().click();
  if (info.project.name === 'phone') await page.getByRole('menuitem', { name: 'Install App' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await shot(page, info, 'sheet-install');
  await page.keyboard.press('Escape');
  await open(page, '/macs/studio');
  await page.getByRole('button', { name: /Power options/ }).click();
  await shot(page, info, 'menu-power');
  await page.getByRole('menuitem', { name: 'Force Shut Down' }).click();
  await shot(page, info, 'confirm-force-shutdown');
});
