import { expect, test } from '@playwright/test';
import { open, shot, typeCode } from './helpers';

test('creates a token after confirming the password, then copies it once', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page, '/macs/studio/connect');
  await page.getByRole('button', { name: 'Create Token' }).click();
  await page.getByRole('textbox', { name: 'Name' }).fill('Claude Code on Mac mini');
  await page.getByRole('button', { name: 'Create Token' }).last().click();

  // Sensitive: the server asks for the password first, then the request continues.
  await expect(page.getByRole('heading', { name: 'Confirm It’s You' })).toBeVisible();
  await shot(page, info, 'reauth');
  await page.getByLabel('Password').fill('demo-password-1');
  await typeCode(page, '123456');
  await page.getByRole('button', { name: 'Continue' }).click();

  await expect(page.getByRole('heading', { name: 'Token Created' })).toBeVisible();
  await page.getByRole('button', { name: 'Copy token' }).click();
  await expect(page.getByRole('button', { name: 'token copied' })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('dot_pat_Ab3xSYNTHETICdemoTOKENnotREAL0000');
  // The confirmation resets itself.
  await expect(page.getByRole('button', { name: 'Copy token' })).toBeVisible({ timeout: 4000 });
  await shot(page, info, 'token-created');
  await page.getByRole('button', { name: 'I Saved It' }).click();
  await expect(page.getByText('Claude Code on Mac mini')).toBeVisible();
});

test('cancelling the confirmation leaves things as they were', async ({ page }) => {
  await open(page, '/macs/studio/settings');
  await page.getByRole('button', { name: 'Delete Mac…' }).click();
  await page.getByLabel('Type Studio Mac to confirm').fill('Studio Mac');
  await page.getByRole('button', { name: 'Delete Mac' }).click();
  await expect(page.getByRole('heading', { name: 'Confirm It’s You' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('heading', { name: 'Confirm It’s You' })).toBeHidden();
  await expect(page).toHaveURL(/\/macs\/studio\/settings/);
});

test('consent: an assistant asks for a Mac and goes back with the answer', async ({ page }, info) => {
  await open(page, '/connect?request=req-claude', 'many');
  await expect(page.getByRole('heading', { name: 'Allow Claude to use a Mac?' })).toBeVisible();
  await page.getByRole('radio', { name: /Work Mac/ }).click();
  await shot(page, info, 'consent');
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page).toHaveURL(/demo-oauth-return\?client=Claude&approved=1/);
});

test('walks through adding the Mac to Meta Muse with a token kept out of the chat', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page, '/macs/studio/connect');
  await page.getByRole('combobox', { name: 'Assistant' }).click();
  await page.getByRole('option', { name: 'Meta Muse' }).click();

  // The message for Muse has the address and never the key; the key's block waits for a token.
  const message = page.getByRole('figure').filter({ hasText: 'Message to Muse' });
  await expect(message).toContainText('streamable HTTP at https://home.example.test/mcp');
  const key = page.getByRole('figure').filter({ hasText: 'API key' });
  await expect(key.getByText('Create the token first')).toBeVisible();

  await page.getByRole('button', { name: 'Create Token for Meta Muse' }).click();
  await page.getByLabel('Password').fill('demo-password-1');
  await typeCode(page, '123456');
  await page.getByRole('button', { name: 'Continue' }).click();

  await key.getByRole('button', { name: 'Copy API key' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Bearer dot_pat_Ab3xSYNTHETICdemoTOKENnotREAL0000');
  await expect(message).not.toContainText('dot_pat_');
  await expect(page.getByRole('heading', { name: 'Personal access tokens' }).locator('..').locator('..')).toContainText('Meta Muse');
  await shot(page, info, 'connect-muse', true);
});
