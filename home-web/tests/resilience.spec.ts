import { expect, test } from '@playwright/test';
import { open, shot } from './helpers';

test('shows a calm offline note and recovers', async ({ page }, info) => {
  await open(page, '/macs/studio');
  await expect(page.getByText('Ready').first()).toBeVisible();
  await page.evaluate(() => window.__demo!.setOffline(true));
  await page.getByRole('switch', { name: 'Pause assistant' }).click();
  await expect(page.getByText('Can’t reach Dot Home.', { exact: true })).toBeVisible();
  await shot(page, info, 'offline');
  await page.evaluate(() => window.__demo!.setOffline(false));
  await page.getByRole('button', { name: 'Try Again' }).click();
  await expect(page.getByText('Can’t reach Dot Home.', { exact: true })).toBeHidden();
});

test('a server that cannot be reached at all gets a full-screen retry', async ({ page }, info) => {
  await open(page, '/', 'offline');
  await expect(page.getByRole('heading', { name: 'Can’t reach Dot Home' })).toBeVisible();
  await shot(page, info, 'offline-start');
});

test('activity streams in live and pages back through history', async ({ page }) => {
  await open(page, '/macs/studio/activity');
  const rows = page.locator('main li');
  await expect(rows.first()).toBeVisible();
  const before = await rows.count();
  await page.evaluate(() => window.__demo!.activity('studio', 'error'));
  await expect(rows).toHaveCount(before + 1);
  await page.getByRole('button', { name: 'Errors' }).click();
  await expect(page.locator('main li').first()).toContainText(/TIMEOUT|PERMISSION_DENIED/);
  await page.getByRole('button', { name: 'All', exact: true }).click();
  await page.mouse.wheel(0, 20_000);
  await expect(page.getByText('That’s everything Dot Home keeps for this Mac.')).toBeVisible({ timeout: 10_000 });
});

test('the live stream reconnects after it drops', async ({ page }) => {
  await open(page, '/macs/studio');
  await page.evaluate(() => window.__demo!.dropStreams());
  await page.waitForTimeout(2500);
  await page.evaluate(() => window.__demo!.approval('studio'));
  await expect(page.getByRole('article')).toHaveCount(2);
});

test('the live screen opens view only and hands over control on request', async ({ page }, info) => {
  await open(page, '/macs/studio/screen');
  await expect(page.locator('canvas').first()).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText(/^View only\./)).toBeVisible();
  await page.getByRole('button', { name: /Control/ }).click();
  await expect(page.getByText(/You’re in control/)).toBeVisible();
  await shot(page, info, 'screen-control');
});
