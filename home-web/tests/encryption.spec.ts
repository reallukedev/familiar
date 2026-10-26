import { expect, test } from '@playwright/test';
import { open, saveRecoveryKey, shot, typeCode } from './helpers';

test('a locked Mac shows Locked and unlocks from its page', async ({ page }, info) => {
  await open(page, '/macs/studio', 'locked');
  await expect(page.getByText('Sign in to unlock.').first()).toBeVisible();
  await shot(page, info, 'locked-mac');
  await page.getByRole('button', { name: 'Unlock…' }).first().click();
  await page.getByLabel('Password').fill('demo-password-1');
  await typeCode(page, '123456');
  await page.getByRole('button', { name: 'Unlock', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Start Mac' }).first()).toBeVisible();
});

test('encrypts an older Mac from its Settings, with progress', async ({ page }, info) => {
  await open(page, '/macs/old/settings', 'recovery');
  await page.getByRole('button', { name: 'Encrypt This Mac…' }).click();
  await expect(page.getByText('Its snapshots are removed.')).toBeVisible();
  await shot(page, info, 'encrypt-sheet');
  await page.getByRole('button', { name: 'Encrypt', exact: true }).click();
  await expect(page.getByText('Encrypting…').first()).toBeVisible();
  await shot(page, info, 'encrypting');
  await expect(page.getByText('Its disk, snapshots and saved memory are encrypted with your password.')).toBeVisible({ timeout: 15_000 });
});

test('an account without a saved recovery key makes a new one and saves it', async ({ page }, info) => {
  await open(page, '/', 'recovery');
  await page.getByRole('button', { name: 'Show Recovery Key…' }).click();
  // The session's confirmation may have lapsed: confirm the password if asked.
  const confirm = page.getByRole('heading', { name: 'Confirm It’s You' });
  await expect(confirm.or(page.getByRole('heading', { name: 'Your Recovery Key' }))).toBeVisible();
  if (await confirm.isVisible()) {
    await page.getByLabel('Password').fill('demo-password-1');
    await typeCode(page, '123456');
    await page.getByRole('button', { name: 'Continue' }).click();
  }
  await expect(page.getByRole('heading', { name: 'Your Recovery Key' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await shot(page, info, 'recovery-key');
  await saveRecoveryKey(page);
  await expect(page.getByRole('button', { name: 'Show Recovery Key…' })).toBeHidden();
});
