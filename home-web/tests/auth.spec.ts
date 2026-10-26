import { expect, test } from '@playwright/test';
import { open, saveRecoveryKey, shot, typeCode } from './helpers';

test('signs in with a password and an authenticator code', async ({ page }, info) => {
  await open(page, '/macs/studio', 'signedout');
  await expect(page).toHaveURL(/\/signin\?next=%2Fmacs%2Fstudio/);
  await shot(page, info, 'signin');
  await page.getByLabel('Username').fill('alex');
  await page.getByLabel('Password').fill('wrong-password');
  await page.getByRole('button', { name: 'Sign In' }).click();
  await expect(page.getByRole('alert')).toContainText('don’t match');
  await page.getByLabel('Password').fill('demo-password-1');
  await page.getByRole('button', { name: 'Sign In' }).click();
  await expect(page.getByRole('heading', { name: 'Enter your code' })).toBeVisible();
  await typeCode(page, '000000');
  await expect(page.getByRole('alert')).toContainText('isn’t right');
  await shot(page, info, 'totp-wrong');
  await typeCode(page, '123456');
  // Back where the person was going.
  await expect(page).toHaveURL(/\/macs\/studio(\?|$)/);
  await expect(page.getByText('Ready').first()).toBeVisible();
});

test('first run creates the admin, then requires two-factor before anything else', async ({ page }, info) => {
  await open(page, '/setup?token=demo-setup', 'firstrun');
  await shot(page, info, 'setup');
  await page.getByRole('button', { name: 'Create Admin Account' }).click();
  await expect(page.getByText('Choose a username.')).toBeVisible();
  await page.getByLabel('Your name').fill('Riley Chen');
  await page.getByLabel('Username').fill('riley');
  await page.getByLabel('Password', { exact: true }).fill('a long synthetic passphrase');
  await page.getByLabel('Confirm password').fill('a long synthetic passphrase');
  await page.getByRole('button', { name: 'Create Admin Account' }).click();
  await saveRecoveryKey(page);
  await expect(page.getByRole('heading', { name: /Turn on two.factor/ })).toBeVisible();
  await expect(page.getByRole('img', { name: /QR code/ })).toBeVisible();
  await shot(page, info, 'enroll');
  await typeCode(page, '123456');
  await expect(page.getByRole('heading', { name: 'Your own Mac, for your assistant' })).toBeVisible();
});

test('joins from an invite link', async ({ page }) => {
  await open(page, '/invite/demo-join', 'signedout');
  await page.getByLabel('Your name').fill('Casey Lin');
  await page.getByLabel('Username').fill('casey');
  await page.getByLabel('Password').fill('another synthetic passphrase');
  await page.getByRole('button', { name: 'Create Account' }).click();
  await saveRecoveryKey(page);
  await expect(page.getByRole('button', { name: 'Create Your Mac' })).toBeVisible();
});

test('a reset link needs the recovery key, or erasing the encrypted Macs', async ({ page }) => {
  await open(page, '/invite/demo-reset', 'signedout');
  await page.getByLabel('New password').fill('a brand new synthetic passphrase');
  await page.getByLabel('Recovery key').fill('demo rcvr key2 3456 abcd efgh jklm npqz');
  await page.getByRole('button', { name: 'Change Password' }).click();
  await expect(page.getByText('That recovery key isn’t right.', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Don’t Have It?' }).click();
  await expect(page.getByRole('button', { name: 'Erase and Reset Password' })).toBeDisabled();
  await page.getByRole('button', { name: 'Use My Recovery Key' }).click();
  await page.getByLabel('Recovery key').fill('demo rcvr key2 3456 abcd efgh jklm npqr');
  await page.getByRole('button', { name: 'Change Password' }).click();
  await expect(page.getByRole('heading', { name: /Your Macs?/ })).toBeVisible();
});

test('a reset for a two-factor account finishes with the code on the same page', async ({ page }) => {
  await open(page, '/invite/demo-reset-2fa', 'signedout');
  await page.getByLabel('New password').fill('a brand new synthetic passphrase');
  await page.getByRole('button', { name: 'Don’t Have It?' }).click();
  await page.getByLabel('Erase my encrypted Macs').check();
  await page.getByRole('button', { name: 'Erase and Reset Password' }).click();
  await expect(page.getByRole('heading', { name: 'Enter your code' })).toBeVisible();
  await typeCode(page, '000000');
  await expect(page.getByRole('alert')).toContainText('isn’t right');
  await typeCode(page, '123456');
  await saveRecoveryKey(page);
  await expect(page).toHaveURL(/\/(\?|$)/);
});

test('locked Macs unlock with the password', async ({ page }) => {
  await open(page, '/', 'locked');
  await expect(page.getByText('Locked').first()).toBeVisible();
  await page.getByRole('button', { name: 'Unlock…' }).first().click();
  await expect(page.getByRole('heading', { name: 'Unlock Your Macs' })).toBeVisible();
  await page.getByLabel('Password').fill('demo-password-1');
  await typeCode(page, '123456');
  await page.getByRole('button', { name: 'Unlock', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Unlock Your Macs' })).toBeHidden();
  await expect(page.getByText('Macs are locked')).toBeHidden();
  await page.goto('/macs/studio?demo&scenario=locked');
  await expect(page.getByRole('heading', { name: 'Studio Mac' }).first()).toBeVisible();
});

test('an expired invite says what to do', async ({ page }) => {
  await open(page, '/invite/old-link', 'signedout');
  await expect(page.getByRole('heading', { name: 'This link no longer works' })).toBeVisible();
});
