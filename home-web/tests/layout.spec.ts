import { expect, test } from '@playwright/test';
import { noHorizontalOverflow, open } from './helpers';

const WIDTHS = [320, 390, 768, 1024, 1440, 2560];
const ROUTES: [string, string][] = [
  ['/', 'many'],
  ['/', 'empty'],
  ['/macs/studio', 'errors'],
  ['/macs/long', 'longnames'],
  ['/macs/studio/apps', 'longnames'],
  ['/macs/studio/activity', 'one'],
  ['/macs/studio/permissions', 'one'],
  ['/macs/studio/connect', 'one'],
  ['/macs/studio/snapshots', 'one'],
  ['/macs/studio/settings', 'one'],
  ['/macs/studio/screen', 'one'],
  ['/account', 'one'],
  ['/admin', 'admin'],
  ['/admin/users', 'admin'],
  ['/admin/image', 'admin'],
  ['/admin/updates', 'updates-available'],
  ['/admin/updates', 'updates-nightly'],
  ['/macs/work/settings', 'updates-available'],
  ['/signin', 'signedout'],
  ['/connect?request=req-chatgpt', 'many'],
];

test.describe('no sideways scrolling', () => {
  test.beforeEach(({}, info) => test.skip(info.project.name !== 'desktop', 'widths are set explicitly'));
  for (const width of WIDTHS) {
    test(`at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      for (const [path, scenario] of ROUTES) {
        await open(page, path, scenario);
        await page.waitForTimeout(400);
        await noHorizontalOverflow(page).catch((error: Error) => {
          throw new Error(`${path} (${scenario}) at ${width}: ${error.message}`);
        });
      }
    });
  }
});

test('touch targets on the phone are at least 44 points', async ({ page }, info) => {
  test.skip(info.project.name !== 'phone');
  for (const [path, scenario] of [['/macs/studio', 'one'], ['/macs/studio/connect', 'one'], ['/account', 'one'], ['/admin/updates', 'updates-available'], ['/macs/studio/settings', 'updates-available']]) {
    await open(page, path!, scenario);
    await page.waitForTimeout(400);
    const small = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>('a[href], button, [role="switch"], input')]
        .filter((el) => el.offsetParent !== null && !el.closest('[aria-hidden="true"]'))
        .map((el) => {
          const box = el.getBoundingClientRect();
          // Text links inside running copy and inline code-copy buttons are exempt.
          const inline = el.tagName === 'A' && el.closest('p, li');
          return { label: el.getAttribute('aria-label') || el.textContent?.trim() || el.tagName, w: box.width, h: box.height, inline };
        })
        .filter((item) => !item.inline && (item.h < 30 || item.w < 30)),
    );
    expect(small, `${path}: ${JSON.stringify(small)}`).toEqual([]);
  }
});
