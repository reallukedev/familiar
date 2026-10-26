import { expect, type Page, type TestInfo } from '@playwright/test';

/** Open a route against the synthetic demo backend in a given scenario. */
export async function open(page: Page, path: string, scenario = 'one') {
  const sep = path.includes('?') ? '&' : '?';
  await page.goto(`${path}${sep}demo&scenario=${scenario}`);
  await expect(page.locator('#root')).not.toBeEmpty();
}

/** Save a screenshot under test-results/screens, named by project. */
export async function shot(page: Page, info: TestInfo, name: string, fullPage = false) {
  await page.waitForTimeout(250);
  await page.screenshot({ path: `test-results/screens/${info.project.name}-${name}.png`, fullPage, animations: 'disabled' });
}

export const isPhone = (info: TestInfo) => info.project.name === 'phone';

/** Fill a six-digit code field (Base UI OTP field: one input per digit). */
export async function typeCode(page: Page, code: string) {
  await page.getByRole('textbox', { name: /Six-digit code|Code/ }).first().click();
  await page.keyboard.type(code);
}

/** A new account shows its recovery key once; tick Saved It and continue. */
export async function saveRecoveryKey(page: Page) {
  await expect(page.getByRole('heading', { name: 'Your Recovery Key' })).toBeVisible();
  await page.getByLabel('I saved my recovery key').check();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByRole('heading', { name: 'Your Recovery Key' })).toBeHidden();
}

export async function noHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow, 'page scrolls sideways').toBeLessThanOrEqual(0);
}
