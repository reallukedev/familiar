import { expect, test, type Page } from '@playwright/test';
import { isPhone, open, shot, typeCode } from './helpers';

/** Pick from a Choice: a pop-up menu on a Mac, a sheet with a checked list on a phone. */
async function choose(page: Page, label: string, option: string) {
  if (isPhone(test.info())) {
    await page.getByRole('button', { name: new RegExp(`^${label}:`) }).click();
    await page.getByRole('radio', { name: new RegExp(`^${option}`) }).click();
    await expect(page.getByRole('radiogroup')).toBeHidden();
  } else {
    await page.getByRole('combobox', { name: label }).click();
    await page.getByRole('option', { name: option, exact: true }).click();
  }
}

async function confirmPassword(page: Page) {
  await expect(page.getByRole('heading', { name: 'Confirm It’s You' })).toBeVisible();
  await page.getByLabel('Password').fill('demo-password-1');
  await typeCode(page, '123456');
  await page.getByRole('button', { name: 'Continue' }).click();
}

const card = (page: Page) => page.getByRole('main').locator('section').first();

test('installs an update, waits out the restart, and shows the new version', async ({ page }, info) => {
  await open(page, '/admin/updates', 'updates-available');
  await expect(page.getByRole('heading', { name: 'Dot Home 3.1.0 is available' })).toBeVisible();
  await expect(page.getByText('Snapshots show how much space each one uses.')).toBeVisible();
  await shot(page, info, 'updates-available', true);

  await page.getByRole('button', { name: 'Update Now' }).click();
  await confirmPassword(page);
  await expect(card(page).getByRole('heading', { name: /^(Downloading|Installing) Dot Home 3\.1\.0$/ })).toBeVisible();
  await expect(card(page).getByRole('progressbar')).toBeVisible();

  await expect(page.getByRole('heading', { name: 'Restarting Dot Home…' })).toBeVisible({ timeout: 10_000 });
  await shot(page, info, 'updates-restarting');
  // An expected restart isn't an outage: no offline banner.
  await expect(page.getByText('Can’t reach Dot Home.')).toHaveCount(0);

  await expect(page.getByRole('heading', { name: 'Dot Home is up to date' })).toBeVisible({ timeout: 15_000 });
  await expect(card(page)).toContainText('Dot Home 3.1.0 · Stable');
  await expect(page.getByText('Dot Home 3.1.0 is running')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Roll Back to 3.0.0…' })).toBeVisible();
  if (!isPhone(info)) await expect(page.getByText('Dot Home 3.1.0', { exact: true })).toBeVisible();
  await shot(page, info, 'updates-installed', true);
});

test('changes the channel, the automatic install and its window', async ({ page }) => {
  await open(page, '/admin/updates', 'updates');
  await expect(page.getByRole('heading', { name: 'Dot Home is up to date' })).toBeVisible();
  await expect(page.getByText('New versions install between 3:00 and 5:00 AM, server time.', { exact: false })).toBeVisible();

  // Settings need the password; the change goes through once it's confirmed.
  await choose(page, 'Install window', '11:00 PM to 1:00 AM');
  await confirmPassword(page);
  await expect(page.getByText('between 11:00 PM and 1:00 AM', { exact: false })).toBeVisible();

  await page.getByRole('switch', { name: 'Install updates automatically' }).click();
  await expect(page.getByRole('switch', { name: 'Install updates automatically' })).not.toBeChecked();
  await expect(page.getByText('Install window')).toHaveCount(0);
  await expect(page.getByText('You choose when to install.', { exact: false })).toBeVisible();

  await choose(page, 'Update channel', 'Nightly');
  await expect(page.getByRole('heading', { name: /^Dot Home 3\.2\.0-nightly\.\d{8} is available$/ })).toBeVisible();
  await expect(page.getByText('The newest changes from every commit. Can break.').first()).toBeVisible();

  await choose(page, 'Update channel', 'Off');
  await expect(page.getByRole('heading', { name: 'Updates are off' })).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Install updates automatically' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Check Now/ })).toHaveCount(0);
});

test('leaving nightly offers the older stable build with a warning', async ({ page }, info) => {
  await open(page, '/admin/updates', 'updates-nightly');
  await expect(card(page)).toContainText(/You have 3\.2\.0-nightly\.\d{8} \(c41de09\)/);
  await choose(page, 'Update channel', 'Stable');
  await confirmPassword(page);
  await expect(page.getByRole('heading', { name: 'Stable has Dot Home 3.1.0' })).toBeVisible();
  await expect(card(page)).toContainText('older than the nightly build you run now');
  await expect(page.getByRole('button', { name: 'Install Stable' })).toBeVisible();
  await shot(page, info, 'updates-switch-to-stable');
});

test('rolls back after a confirmation and the password', async ({ page }, info) => {
  await open(page, '/admin/updates', 'updates');
  await page.getByRole('button', { name: 'Roll Back to 3.0.0…' }).click();
  const dialog = page.getByRole('alertdialog');
  await expect(dialog).toContainText('Roll back to Dot Home 3.0.0?');
  await shot(page, info, 'updates-rollback-confirm');
  await dialog.getByRole('button', { name: 'Roll Back' }).click();
  await confirmPassword(page);
  await expect(page.getByRole('heading', { name: 'Restarting Dot Home…' })).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('heading', { name: 'Dot Home 3.1.0 is available' })).toBeVisible({ timeout: 15_000 });
  await expect(card(page)).toContainText('You have 3.0.0.');
  const history = page.getByRole('main').locator('section').filter({ has: page.getByRole('heading', { name: 'History' }) });
  await expect(history.getByText('Rolled back').first()).toBeVisible();
  await expect(history).toContainText('Rolled back from the portal.');
});

test('a Mac’s agent follows its own channel and updates on request', async ({ page }, info) => {
  await open(page, '/macs/studio', 'updates-available');
  await expect(page.getByText('Update available')).toBeVisible();

  await open(page, '/macs/studio/settings', 'updates-available');
  const agent = page.getByRole('region', { name: 'Agent updates' });
  await expect(agent).toContainText('3.0.0');
  await expect(agent).toContainText('Agent 3.1.0 is available');
  await choose(page, 'Agent channel', 'Nightly');
  await expect(agent).toContainText(/Agent 3\.2\.0-nightly\.\d{8} is available/);
  await choose(page, 'Agent channel', 'Use Server Setting');
  await expect(agent.getByRole(isPhone(info) ? 'button' : 'combobox', { name: /Agent channel/ })).toContainText('Use Server Setting (Stable)');

  await agent.getByRole('button', { name: 'Update Now' }).click();
  await expect(agent).toContainText('Updating to 3.1.0…');
  await shot(page, info, 'agent-updating');
  await expect(agent.getByRole('button', { name: 'Update Now' })).toHaveCount(0, { timeout: 8000 });
  await expect(agent).not.toContainText('Updating');
  await expect(agent.getByText('3.1.0', { exact: true })).toBeVisible();
});

test('an agent too old to update itself shows how to reinstall it', async ({ page }, info) => {
  await open(page, '/macs/work/settings', 'updates-available');
  const agent = page.getByRole('region', { name: 'Agent updates' });
  await expect(agent).toContainText('too old to update itself');
  await expect(agent.getByText('curl -fsSL https://home.example.test/guest/install.sh | sh')).toBeVisible();
  await expect(agent.getByRole('button', { name: 'Update Now' })).toHaveCount(0);
  await shot(page, info, 'agent-reinstall', true);
});

test('a build without updates explains why and hides the controls', async ({ page }, info) => {
  await open(page, '/admin/updates', 'updates-disabled');
  await expect(page.getByRole('heading', { name: 'Updates aren’t available for this build' })).toBeVisible();
  await expect(page.getByText('This build wasn’t made by the release pipeline, so it can’t verify updates.')).toBeVisible();
  await expect(page.getByText('Update channel')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Check Now|Roll Back/ })).toHaveCount(0);
  await shot(page, info, 'updates-disabled');
});

test('the restarting scenario walks through to the new version on its own', async ({ page }) => {
  await open(page, '/admin/updates', 'updates-restarting');
  await expect(page.getByRole('heading', { name: 'Restarting Dot Home…' })).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('heading', { name: 'Dot Home is up to date' })).toBeVisible({ timeout: 15_000 });
});
