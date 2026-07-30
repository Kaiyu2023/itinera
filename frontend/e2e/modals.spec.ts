import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';

/**
 * Modal chrome, which every composer in the app was missing.
 *
 * All four failures are invisible with a mouse, which is why they survived so
 * long: opening a composer left focus on the trigger *behind* the backdrop, so
 * the first Tab walked the cards underneath it; the page scrolled under an open
 * sheet on a phone; and the dialog announced itself as "dialog".
 */

/** Where focus is, and whether it is inside the open dialog. */
async function focusState(page: Page, dialog: string) {
  return page.evaluate((sel) => {
    const el = document.activeElement as HTMLElement | null;
    return {
      inside: !!el && !!document.querySelector(sel)?.contains(el),
      tag: el?.tagName ?? null,
      bodyOverflow: getComputedStyle(document.body).overflow,
    };
  }, dialog);
}

test('a governance sheet takes focus, keeps it, and gives it back', async ({ page }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline&day=d6');

  const opener = page.getByRole('button', { name: /Propose a stop on this day/ });
  await opener.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();

  // Named, not just "dialog".
  await expect(dialog).toHaveAttribute('aria-labelledby', 'gov-modal-title');
  await expect(page.locator('#gov-modal-title')).toContainText('Propose a stop');

  const opened = await focusState(page, '[role="dialog"]');
  expect(opened.inside).toBe(true);
  expect(opened.bodyOverflow).toBe('hidden');

  // Twenty tabs is more than any of these surfaces has controls, so if the trap
  // leaks at all this catches it.
  for (let i = 0; i < 20; i++) {
    await page.keyboard.press('Tab');
    expect((await focusState(page, '[role="dialog"]')).inside, `escaped on tab ${i + 1}`).toBe(true);
  }
  await page.keyboard.press('Shift+Tab');
  expect((await focusState(page, '[role="dialog"]')).inside).toBe(true);

  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  expect((await focusState(page, '[role="dialog"]')).bodyOverflow).not.toBe('hidden');
  await expect(opener).toBeFocused();
});

test('the ledger composer locks the page behind it', async ({ page }) => {
  await page.goto('/trips/t-japan26/ledger');
  await page
    .getByRole('button', { name: /Add expense/ })
    .first()
    .click();
  await expect(page.locator('.exp-modal')).toBeVisible();

  const state = await focusState(page, '.exp-modal');
  expect(state.inside).toBe(true);
  expect(state.bodyOverflow).toBe('hidden');

  // Wheeling over the backdrop used to scroll the trip page underneath.
  await page.mouse.wheel(0, 600);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);

  await page.keyboard.press('Escape');
  await expect(page.locator('.exp-modal')).toHaveCount(0);
  expect(await page.evaluate(() => getComputedStyle(document.body).overflow)).not.toBe('hidden');
});

test('a disabled primary action does not look like a live one', async ({ page }) => {
  // `opacity: 0.5` on a solid accent fill reads as enabled and silently
  // swallows the click.
  await page.goto('/trips/t-japan26/plan?view=timeline&gov=discuss&stop=s-d1-omoide');
  const start = page.getByRole('button', { name: 'Start' });
  await expect(start).toBeDisabled();

  const style = await start.evaluate((el) => {
    const s = getComputedStyle(el);
    return { opacity: s.opacity, cursor: s.cursor, background: s.backgroundColor };
  });
  expect(style.opacity).toBe('1');
  expect(style.cursor).toBe('not-allowed');

  const accent = await page.evaluate(() => getComputedStyle(document.body).getPropertyValue('--accent').trim());
  expect(style.background).not.toBe(accent);
});
