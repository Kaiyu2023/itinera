import { test, expect } from '@playwright/test';

/**
 * The two things the app could describe but not change: what theme you are
 * looking at, and what phase a trip is in.
 *
 * Both were states the type system already declared. Dark mode existed only as
 * a media query, so on a laptop pinned to light there was no way to see it;
 * `TripStatus` had five values and the UI rendered one of them as a static
 * pill. A state you can read and cannot set is a state the product does not
 * really have.
 */

test.describe('colour theme', () => {
  // The OS says light for every test in this block, so anything dark below is
  // the switch overriding it — which is the whole point of the feature.
  test.use({ colorScheme: 'light' });

  test('the switch overrides the OS, and the choice survives a reload', async ({ page }) => {
    await page.goto('/trips/t-japan26/plan?view=timeline');
    const theme = () => page.evaluate(() => document.documentElement.dataset.theme);
    const bg = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);

    expect(await theme()).toBe('light');
    const lightBg = await bg();

    await page.getByRole('radio', { name: 'Dark' }).click();
    expect(await theme()).toBe('dark');
    expect(await bg()).not.toBe(lightBg);

    await page.reload();
    // Set by the blocking inline script in index.html, before any CSS applies —
    // so there is no flash of the wrong theme on the way in.
    expect(await theme()).toBe('dark');
    await expect(page.getByRole('radio', { name: 'Dark' })).toHaveAttribute('aria-checked', 'true');

    // Auto hands control back to the device, which here says light.
    await page.getByRole('radio', { name: 'Auto' }).click();
    expect(await theme()).toBe('light');
    await page.reload();
    expect(await theme()).toBe('light');
  });

  test('a hand-darkened page synthesises its accent for a dark substrate', async ({ page }) => {
    // The accent keeps only the trip photo's hue and rebuilds lightness per
    // theme. Before the switch existed that read `prefers-color-scheme`
    // directly, so a page darkened by hand kept an accent built at L=0.52 for a
    // cream page — legible against the wrong background by luck, not
    // construction.
    await page.goto('/trips/t-aegean27/candidates');
    const accent = () => page.evaluate(() => getComputedStyle(document.body).getPropertyValue('--accent').trim());
    const inLight = await accent();

    await page.getByRole('radio', { name: 'Dark' }).click();
    await expect.poll(accent).not.toBe(inLight);
  });
});

test.describe('trip phase', () => {
  test('the hero pill moves the trip along its lifecycle', async ({ page }) => {
    await page.goto('/trips/t-japan26/plan?view=timeline');
    const pill = page.getByRole('button', { name: /Trip phase/ });
    await expect(pill).toHaveText(/Planning/);

    await pill.click();
    const menu = page.getByRole('menu', { name: 'Trip phase' });
    await expect(menu).toBeVisible();
    // Portalled out of the hero, which is `overflow: hidden` for its cover
    // photo's rounded corners and used to slice the panel off two rungs down.
    const box = (await menu.boundingBox())!;
    const width = page.viewportSize()!.width;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width + 1);

    await page.getByRole('menuitem', { name: /Booked/ }).click();
    await expect(pill).toHaveText(/Booked/);
    await expect(menu).toHaveCount(0);
  });

  test('the phase changes how loud the page is allowed to be', async ({ page }) => {
    // `--env-amplitude` is keyed to status: a trip you are dreaming about can
    // look like weather, an itinerary read on a train cannot. Making the phase
    // settable is what turns that from a fixture property into a control.
    await page.goto('/trips/t-japan26/plan?view=timeline');
    const amplitude = () =>
      page.evaluate(() => parseFloat(getComputedStyle(document.body).getPropertyValue('--env-amplitude')));
    // Polled: the amplitude lands on <body> from an effect that waits on the
    // trip query, so it is briefly the default 1 on first paint.
    await expect.poll(amplitude).toBeCloseTo(0.75, 2);

    await page.getByRole('button', { name: /Trip phase/ }).click();
    await page.getByRole('menuitem', { name: /Dreaming/ }).click();
    await expect.poll(amplitude).toBeCloseTo(1, 2);

    await page.getByRole('button', { name: /Trip phase/ }).click();
    await page.getByRole('menuitem', { name: /Booked/ }).click();
    await expect.poll(amplitude).toBeCloseTo(0.3, 2);
  });

  test('backwards is a legal move', async ({ page }) => {
    // Bookings fall through and dates slip. The moment that happens is the
    // moment you least want the app arguing with you.
    await page.goto('/trips/t-japan26/plan?view=timeline');
    const pill = page.getByRole('button', { name: /Trip phase/ });
    await pill.click();
    await page.getByRole('menuitem', { name: /Done/ }).click();
    await expect(pill).toHaveText(/Done/);

    await pill.click();
    await page.getByRole('menuitem', { name: /Dreaming/ }).click();
    await expect(pill).toHaveText(/Dreaming/);
  });
});
