import { test, expect } from '@playwright/test';

/**
 * The plan is depicted, not described.
 *
 * These lock the properties the redesign exists for. The old timeline rendered
 * a four-hour temple visit and a five-minute walk as rows of identical height
 * and told you the day was "87% full" in a badge; every assertion here would
 * have failed against it, which is the point.
 */

const TRIP = '/trips/t-japan26';
const DAY6 = `${TRIP}/plan?view=timeline&day=d6`;

test('a stop occupies space in proportion to its duration', async ({ page }) => {
  await page.goto(DAY6);

  // Day 6: Fushimi Inari 2h30, Kiyomizu-dera 1h40, Yoshimura 1h.
  const box = async (name: string) => {
    const el = page.locator('.dc-blk').filter({ hasText: name }).first();
    await expect(el).toBeVisible();
    return (await el.boundingBox())!;
  };
  const fushimi = await box('Fushimi Inari');
  const kiyomizu = await box('Kiyomizu-dera');
  const yoshimura = await box('Yoshimura');

  // 150 / 100 / 60 minutes. Allow a little slack for borders and rounding, but
  // the ratios have to actually hold — that is the whole claim.
  expect(fushimi.height / yoshimura.height).toBeGreaterThan(2.2);
  expect(fushimi.height / yoshimura.height).toBeLessThan(2.8);
  expect(kiyomizu.height / yoshimura.height).toBeGreaterThan(1.4);
  expect(kiyomizu.height / yoshimura.height).toBeLessThan(1.9);
});

test('a stop that runs past sunset is drawn in the dark', async ({ page }) => {
  await page.goto(DAY6);

  // The fixture note says "the 14:45 Arashiyama arrival leaves little daylight
  // for the grove". That should be something you can see, not something you
  // have to read.
  const grove = page.locator('.dc-blk').filter({ hasText: 'Bamboo Grove' }).first();
  await expect(grove).toHaveClass(/after-dark/);
  await expect(grove.getByText('after dark')).toBeVisible();

  // ...and it must genuinely sit below the sunset rule.
  const sunset = page.locator('.dc-sunset');
  await expect(sunset).toBeVisible();
  const groveBox = (await grove.boundingBox())!;
  const sunsetBox = (await sunset.boundingBox())!;
  expect(groveBox.y + groveBox.height).toBeGreaterThan(sunsetBox.y);

  // An earlier stop on the same day must not be flagged.
  await expect(page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' }).first()).not.toHaveClass(/after-dark/);
});

test('unused window time is shown as space, not as a percentage', async ({ page }) => {
  await page.goto(DAY6);
  // Day 6 uses 575 of 660 minutes. The remainder is drawn.
  await expect(page.locator('.dc-tail')).toBeVisible();
  await expect(page.locator('.dc-tail')).toContainText('unplanned');
});

test('only a day with a problem earns a verdict badge', async ({ page }) => {
  // Silence is the signal: colour's one job is the alarm channel, so a day that
  // fits must not spend it. Day 6 is `tight`, Day 7 is `ok`.
  await page.goto(DAY6);
  await expect(page.locator('.day-verdict')).toHaveText('tight');

  await page.goto(`${TRIP}/plan?view=timeline&day=d7`);
  await expect(page.locator('.day-verdict')).toHaveCount(0);
});

test('stop kind is carried by a labelled glyph, not by colour alone', async ({ page }) => {
  await page.goto(DAY6);
  const meal = page.locator('.dc-blk').filter({ hasText: 'Yoshimura' }).first();
  // The kind reaches a screen reader and a dichromat alike.
  await expect(meal.getByRole('img', { name: 'meal' })).toBeVisible();

  const visit = page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' }).first();
  await expect(visit.getByRole('img', { name: 'visit' })).toBeVisible();
});

test('the ribbon shows the whole trip and drives the day view', async ({ page }) => {
  await page.goto(`${TRIP}/plan?view=timeline`);
  const ribbon = page.getByRole('region', { name: 'Whole trip at a glance' });
  await expect(ribbon).toBeVisible();

  const segments = ribbon.locator('.rb-day');
  await expect(segments).toHaveCount(7);

  // A day with a problem is flagged on the overview, so you can see it without
  // opening the day.
  await expect(ribbon.locator('.rb-flag.tight')).toHaveCount(1);

  // Selecting a segment changes the day below it.
  await segments.nth(3).click();
  await expect(segments.nth(3)).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('heading', { name: 'Hakone' })).toBeVisible();
});

test('the ribbon sizes legs by how long they take', async ({ page }) => {
  await page.goto(`${TRIP}/plan?view=timeline`);
  const legs = page.locator('.rb-day').nth(5).locator('.rb-leg');
  await expect(legs.first()).toBeVisible();

  // Day 6's legs are 35, 50 and 5 minutes. The 5-minute walk must be the
  // shortest mark on the line by a wide margin.
  const widths = await legs.evaluateAll((els) => els.map((el) => el.getBoundingClientRect().width));
  expect(widths.length).toBe(3);
  expect(Math.max(...widths)).toBeGreaterThan(Math.min(...widths) * 1.8);
});

test('the canvas is drawn at a real scale, not squeezed to fit', async ({ page, isMobile }) => {
  // The temptation with a proportional view is to compress it until it fits the
  // viewport, at which point it stops being proportional to anything. Day 6 is
  // an eleven-hour window and should be taller than the screen.
  test.skip(!isMobile, 'mobile layout only');
  await page.goto(DAY6);
  const canvas = page.locator('.daycanvas');
  await expect(canvas).toBeVisible();
  const box = (await canvas.boundingBox())!;
  expect(box.height).toBeGreaterThan(page.viewportSize()!.height);
});
