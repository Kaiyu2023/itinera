import { test, expect } from '@playwright/test';

/** The insert-outcome preview round: the add-stop composer shows the day route
    re-drawn through the new stop (dashed accent legs + a "new stop" pin), the
    feasibility warning is projected (current load + the new stop), and the
    ambiguous "Start from" label is now "Choose an idea". */

const TRIP = '/trips/t-japan26';

test('picking a candidate previews the insert on the map', async ({ page, isMobile }) => {
  await page.goto(`${TRIP}/plan?gov=addStop&day=d2&mode=candidates`);
  await expect(page.getByText('Choose an idea', { exact: true })).toBeVisible();
  await expect(page.getByText('Start from')).toHaveCount(0);
  // First shortlisted candidate is preselected → two dashed accent legs splice
  // the new point into the route, and a numbered pin marks where it lands.
  // Routes render into an SVG layer; the basemap is canvas, so dasharray paths
  // can only be the proposed legs. The map pane needs layout to render, so
  // only assert route geometry where the map is actually sized (desktop).
  if (!isMobile) {
    await expect.poll(async () => page.locator('path[stroke-dasharray="2 7"]').count()).toBeGreaterThanOrEqual(2);
    await expect(page.getByText('new stop')).toBeVisible();
  }
});

test('feasibility warning is projected, not current', async ({ page }) => {
  // Day 7 is fine today (84%) but goes tight (~92%) with one more stop — the
  // warning can only appear if the projection includes the proposed insert.
  await page.goto(`${TRIP}/plan?gov=addStop&day=d7&mode=candidates`);
  await expect(page.getByText(/Adding it takes Day 7 to/)).toBeVisible();
  await expect(page.getByText(/~9\d%/)).toBeVisible();
  // Day 1 stays comfortable even with the insert — no warning.
  await page.goto(`${TRIP}/plan?gov=addStop&day=d1&mode=candidates`);
  await expect(page.getByText('Choose an idea', { exact: true })).toBeVisible();
  await expect(page.getByText(/Adding it takes Day 1/)).toHaveCount(0);
});

test('day scrubber fades to the tinted page color, not the raw background', async ({ page }) => {
  await page.goto(`${TRIP}/plan?view=timeline`);
  const scrubber = page.locator('.day-scrubber');
  await expect(scrubber).toBeVisible();
  // Both values resolve through the same computed-style pipeline, so the
  // gradient's first color stop must serialize to the body's tinted color.
  const matches = await scrubber.evaluate((el) => {
    const grad = getComputedStyle(el).backgroundImage;
    const bodyBg = getComputedStyle(document.body).backgroundColor;
    return { grad, bodyBg, ok: grad.includes(bodyBg) };
  });
  expect(matches.ok, `gradient ${matches.grad} should fade from body bg ${matches.bodyBg}`).toBe(true);
});
