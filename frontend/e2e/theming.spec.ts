import { test, expect } from '@playwright/test';

/** The per-trip theming system: every emphasis-role control reads the semantic
    `--accent` token, which TripLayout overrides on <body> from Trip.accentColor.
    The Aegean trip's blue is the proof — before the migration, controls were
    pinned to brand tokens and stayed vermilion on every trip. */

const AEGEAN_BLUE = 'rgb(62, 127, 168)'; // t-aegean27 accentColor #3e7fa8
const BRAND_VERMILION_TOKEN = '#d97b4f'; // --color-accent

const accentOf = (page: import('@playwright/test').Page) =>
  page.evaluate(() => getComputedStyle(document.body).getPropertyValue('--accent').trim());

test('a blue trip themes its controls blue, not brand vermilion', async ({ page }) => {
  await page.goto('/trips/t-aegean27/candidates');
  // TripLayout stamps the accent once the trip loads — poll, don't sample.
  await expect.poll(() => accentOf(page)).toBe('#3e7fa8');
  // A solid-accent control resolves to the trip's blue end-to-end.
  const pitch = page.getByRole('button', { name: /Pitch an idea/ });
  await expect(pitch).toBeVisible();
  expect(await pitch.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe(AEGEAN_BLUE);
});

test('the Japan trip and the trip list resolve to vermilion', async ({ page }) => {
  await page.goto('/trips/t-japan26/candidates');
  await expect.poll(() => accentOf(page)).toBe('#d97b4f');
  // Outside any trip the knob falls back to the brand accent. (Pure style
  // assertions — a fresh load is fine here, nothing was mutated.)
  await page.goto('/');
  await expect(page.locator('body')).not.toHaveClass(/trip-tinted/);
  expect(await accentOf(page)).toBe(BRAND_VERMILION_TOKEN);
});

test('poll vote highlight follows the trip accent', async ({ page }) => {
  // .opt.mine was one of the rules pinned to dusk-blue --color-primary.
  await page.goto('/trips/t-japan26/polls');
  const opt = page.locator('.opt:has(.fill)').first();
  await expect(opt).toBeVisible();
  // color-mix() serialization varies by engine, so compare against a probe
  // element given the expected mix with the accent hex substituted in.
  const { fill, probe } = await opt.locator('.fill').evaluate((el) => {
    const p = document.createElement('div');
    p.style.background = 'color-mix(in srgb, #d97b4f 12%, transparent)';
    document.body.append(p);
    const out = { fill: getComputedStyle(el).backgroundColor, probe: getComputedStyle(p).backgroundColor };
    p.remove();
    return out;
  });
  expect(fill).toBe(probe);
});
