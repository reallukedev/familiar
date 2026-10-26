import { expect, test } from '@playwright/test';
import { isPhone, open, shot } from './helpers';

test('creates a first Mac and watches it start until it is ready', async ({ page }, info) => {
  await open(page, '/', 'empty');
  await shot(page, info, 'macs-empty');
  await page.getByRole('button', { name: 'Create Your Mac' }).click();
  const name = page.getByLabel('Name');
  await expect(name).toHaveValue('Sam’s Mac');
  await name.fill('Lab Mac');
  await shot(page, info, 'new-mac-sheet');
  await page.getByRole('button', { name: 'Create Mac' }).click();
  await expect(page).toHaveURL(/\/macs\/mac-/);
  await page.getByRole('button', { name: 'Start Mac' }).first().click();
  // Live events walk it through starting and booting to ready.
  await expect(page.getByText('Starting macOS').first()).toBeVisible();
  await expect(page.getByText('Ready', { exact: true }).first()).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('img', { name: /Lab Mac screen\. Ready/ })).toBeVisible();
});

test('allows a request the assistant is waiting on', async ({ page }) => {
  await open(page, '/macs/studio', 'approvals');
  const cards = page.getByRole('article');
  await expect(cards).toHaveCount(3);
  await cards.first().getByRole('button', { name: 'Allow' }).click();
  await expect(page.getByText('Allowed')).toBeVisible();
  await expect(cards).toHaveCount(2);
  await cards.first().getByRole('button', { name: 'Deny' }).click();
  await expect(cards).toHaveCount(1);
});

test('a new request arrives live without reloading', async ({ page }) => {
  await open(page, '/macs/studio', 'many');
  await expect(page.getByRole('article')).toHaveCount(2);
  await page.evaluate(() => window.__demo!.approval('studio'));
  await expect(page.getByRole('article')).toHaveCount(3);
  await expect(page.getByText(/wants to/).first()).toBeVisible();
});

test('pauses the assistant with the kill switch', async ({ page }) => {
  await open(page, '/macs/studio');
  const pause = page.getByRole('switch', { name: 'Pause assistant' });
  await pause.click();
  await expect(pause).toBeChecked();
  await expect(page.getByText('Assistant paused').first()).toBeVisible();
});

test('changes a permission and turns on an app', async ({ page }, info) => {
  await open(page, '/macs/studio/permissions');
  const group = page.locator('#group-outgoing');
  await group.getByRole('button', { name: /^Never/ }).click();
  await expect(group.getByRole('button', { name: /^Never/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(group.getByText('Default is Ask.')).toBeVisible();
  await shot(page, info, 'permissions', true);

  await open(page, '/macs/studio/apps');
  const messages = page.getByRole('switch', { name: 'Enable Messages for the assistant' });
  await expect(messages).not.toBeChecked();
  await messages.click();
  await expect(messages).toBeChecked();
  await expect(page.getByText('Messages tools on')).toBeVisible();
});

test('installs an app and maps manifest errors to lines', async ({ page }) => {
  await open(page, '/macs/studio/apps');
  if (isPhone(test.info())) {
    await page.getByRole('button', { name: 'Add apps' }).click();
    await page.getByRole('menuitem', { name: 'Install App' }).click();
  } else await page.getByRole('button', { name: 'Install App' }).click();
  await page.getByRole('button', { name: 'Homebrew' }).click();
  await page.getByLabel('Homebrew cask').fill('firefox-developer-edition');
  await page.getByRole('button', { name: 'Install', exact: true }).click();
  await expect(page.getByText('Installed Firefox Developer Edition')).toBeVisible();

  if (isPhone(test.info())) {
    await page.getByRole('button', { name: 'Add apps' }).click();
    await page.getByRole('menuitem', { name: 'Add Custom Tools' }).click();
  } else await page.getByRole('button', { name: 'Add Custom Tools' }).click();
  const editor = page.getByLabel('Pack manifest (TOML)');
  const text = await editor.inputValue();
  await editor.fill(text.replace('risk = "read"', 'risk = "maybe"'));
  await expect(page.getByRole('button', { name: /Line 11/ })).toBeVisible();
  await editor.fill(text.replace('const args', 'osascript -e "x"; const args'));
  await page.getByRole('button', { name: 'Add Tools' }).click();
  await expect(page.getByText(/Line 17: Scripts can’t start other interpreters/)).toBeVisible();
});

test('finds app servers switched off, and turning one on lists its tools', async ({ page }, info) => {
  await open(page, '/macs/studio/permissions');
  const xcode = page.locator('[id="group-mcp:xcode"]');
  const xcodeMode = xcode.getByRole('combobox', { name: 'Xcode permission' });
  await expect(xcodeMode).toHaveText('Off');
  await xcode.getByRole('button', { name: 'Tools in Xcode' }).click();
  await expect(page.getByText('Choose Ask or Allow to see its tools.')).toBeVisible();
  await page.getByRole('button', { name: 'Done' }).click();

  // An approval mode never turns a server on.
  await page.getByRole('combobox', { name: 'When your assistant waits for you' }).click();
  await page.getByRole('option', { name: 'Allow Everything' }).click();
  await expect(page.locator('#group-observe').getByRole('combobox')).toHaveText('Allow');
  await expect(xcodeMode).toHaveText('Off');

  await xcodeMode.click();
  await page.getByRole('option', { name: 'Allow', exact: true }).click();
  await expect(xcode).toContainText('4 tools.');
  await xcode.getByRole('button', { name: 'Tools in Xcode' }).click();
  await expect(page.getByText('mcp_xcode_BuildProject')).toBeVisible();
  await page.getByRole('button', { name: 'Done' }).click();

  // A server that doesn't answer says what to do.
  const figma = page.locator('[id="group-mcp:figma"]');
  await figma.getByRole('combobox', { name: 'Figma permission' }).click();
  await page.getByRole('option', { name: 'Ask', exact: true }).click();
  await expect(figma).toContainText('Open Figma and turn on its MCP server');
  await shot(page, info, 'permissions-app-servers', true);
});

test('app servers wait for a running Mac', async ({ page }) => {
  await open(page, '/macs/studio/permissions', 'booting');
  await expect(page.getByText('Start the Mac to look for app servers.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Look Again' })).toBeDisabled();
});
