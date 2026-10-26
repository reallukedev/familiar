import { expect, test } from '@playwright/test';
import { isPhone, open, shot } from './helpers';

test('creates an invite link', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page, '/admin/invites', 'admin');
  await page.getByRole('button', { name: 'Create Invite' }).first().click();
  await page.getByRole('button', { name: '1 week' }).click();
  await page.getByRole('button', { name: 'Create Link' }).click();
  await expect(page.getByText('/invite/demo-join')).toBeVisible();
  await page.getByRole('button', { name: 'Copy invite link' }).click();
  await expect(page.getByRole('button', { name: 'invite link copied' })).toBeVisible();
  await shot(page, info, 'invite-created');
});

test('members never see server administration', async ({ page }) => {
  await open(page, '/', 'member');
  await expect(page.getByRole('link', { name: 'Account' }).first()).toBeVisible();
  await expect(page.getByRole('link', { name: /^Server$/ })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'People' })).toHaveCount(0);
  await open(page, '/admin', 'member');
  await expect(page.getByText('Only administrators can open this')).toBeVisible();
});

test('the image builder shows its progress and the steps to finish', async ({ page }, info) => {
  await open(page, '/admin/image', 'admin');
  await expect(page.getByText('Installing macOS', { exact: true })).toBeVisible();
  await expect(page.getByText('dot-guest prepare-image')).toBeVisible();
  await shot(page, info, 'image-builder', true);
  if (!isPhone(info)) {
    await page.getByRole('link', { name: 'Open the Installer’s Screen' }).click();
    await expect(page.locator('canvas').first()).toBeVisible({ timeout: 10_000 });
  }
});
