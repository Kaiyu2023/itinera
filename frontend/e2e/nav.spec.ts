import { test, expect } from '@playwright/test';

/** Back-to-home affordances: the frosted "Trips" pill on the trip hero, the
    quiet pill on the review queue, and (mobile) the chevron in the collapsed
    top bar that appears once the hero scrolls away. */

test('the trip hero carries a frosted pill back to the trip list', async ({ page }) => {
  await page.goto('/trips/t-japan26/plan');
  const back = page.locator('.trip-hero .back-home');
  await expect(back).toBeVisible();
  await expect(back).toHaveAttribute('aria-label', 'Back to all trips');
  await back.click();
  await expect(page).toHaveURL('/');
  await expect(page.getByRole('heading', { name: 'Your trips' })).toBeVisible();
});

test('the review queue carries a quiet pill back home', async ({ page }) => {
  await page.goto('/review');
  const back = page.locator('.rq-page .back-home');
  await expect(back).toBeVisible();
  await back.click();
  await expect(page).toHaveURL('/');
  await expect(page.getByRole('heading', { name: 'Your trips' })).toBeVisible();
});

test('the collapsed mobile bar keeps a back chevron in reach', async ({ page, isMobile }) => {
  test.skip(!isMobile, 'the collapsed trip bar only exists on phones');
  await page.goto('/trips/t-japan26/plan?view=timeline');
  const bar = page.locator('.trip-topbar');
  const back = bar.locator('.back');
  // Collapsed bar (and its chevron) only appears once the hero scrolls away.
  // Wait for the trip to render — scrolling a still-loading (short) page is a
  // no-op. mouse.wheel is also a no-op under touch emulation; programmatic
  // scroll still drives the IntersectionObserver that toggles the bar.
  await expect(page.locator('.trip-hero')).toBeVisible();
  await expect(bar).not.toHaveClass(/visible/);
  await expect
    .poll(async () => {
      await page.evaluate(() => window.scrollTo(0, 2000));
      return page.locator('.trip-topbar.visible').count();
    })
    .toBe(1);
  await expect(back).toBeVisible();
  await back.click();
  await expect(page).toHaveURL('/');
  await expect(page.getByRole('heading', { name: 'Your trips' })).toBeVisible();
});
