import { expect, test, type Page } from '@playwright/test';
import { isPhone, open } from './helpers';

async function unnamedControls(page: Page) {
  return page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>('button, a[href], [role="switch"], [role="radio"], input, textarea, [role="combobox"]')]
      .filter((el) => el.offsetParent !== null || el.getClientRects().length)
      .filter((el) => {
        const labelled = el.getAttribute('aria-label') || el.getAttribute('aria-labelledby') || el.textContent?.trim() || el.getAttribute('title');
        const input = el as HTMLInputElement;
        const labelFor = input.id && document.querySelector(`label[for="${input.id}"]`);
        const hidden = el.closest('[aria-hidden="true"]') || el.getAttribute('tabindex') === '-1';
        return !labelled && !labelFor && !el.closest('label') && input.type !== 'hidden' && !hidden;
      })
      .map((el) => el.outerHTML.slice(0, 120)),
  );
}

test('every control has a name people and screen readers can use', async ({ page }) => {
  for (const [path, scenario] of [['/', 'many'], ['/macs/studio', 'one'], ['/macs/studio/apps', 'one'], ['/macs/studio/permissions', 'one'], ['/macs/studio/connect', 'one'], ['/account', 'one'], ['/admin', 'admin'], ['/admin/updates', 'updates-available'], ['/macs/studio/settings', 'updates-available'], ['/signin', 'signedout']]) {
    await open(page, path!, scenario);
    await page.waitForTimeout(500);
    expect(await unnamedControls(page), path).toEqual([]);
    const images = await page.evaluate(() => [...document.querySelectorAll('img')].filter((img) => !img.hasAttribute('alt')).length);
    expect(images, `${path} images without alt`).toBe(0);
  }
});

test('one main landmark and one page heading', async ({ page }) => {
  await open(page, '/macs/studio/apps');
  await expect(page.getByRole('main')).toHaveCount(1);
  await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
});

test('sheets trap focus, close on Escape and give focus back', async ({ page }) => {
  await open(page, '/macs/studio/snapshots');
  const opener = isPhone(test.info()) ? page.getByRole('button', { name: 'Take snapshot' }) : page.getByRole('button', { name: 'Take Snapshot' }).first();
  await opener.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  for (let i = 0; i < 8; i += 1) {
    await page.keyboard.press('Tab');
    // Base UI's focus guards hand focus back on the next frame.
    await page.waitForTimeout(80);
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
  }
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(opener).toBeFocused();
});

test('the desktop is fully usable from the keyboard', async ({ page }) => {
  test.skip(isPhone(test.info()));
  await open(page, '/macs/studio');
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('combobox')).toBeFocused();
  await page.keyboard.type('perm');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/macs\/studio\/permissions/);
  // "/" also opens search, and a tool name finds its permission group.
  await page.locator('body').press('/');
  await page.keyboard.type('shell');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/permissions#group-shell/);
  // Visible focus ring on the first focusable control.
  await page.keyboard.press('Tab');
  const outline = await page.evaluate(() => getComputedStyle(document.activeElement!).outlineStyle);
  expect(outline).not.toBe('none');
});

test('approvals are announced as they arrive', async ({ page }) => {
  await open(page, '/macs/studio');
  await expect(page.locator('[aria-live="polite"]').filter({ has: page.getByRole('article') })).toHaveCount(1);
});
