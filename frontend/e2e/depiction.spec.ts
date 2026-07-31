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

test('every stop is the same height, the same distance apart', async ({ page }) => {
  await page.goto(DAY6);

  // Day 6: Fushimi Inari 2h30, Kiyomizu-dera 1h40, Yoshimura 1h, Arashiyama
  // 2h30. Under the old linear scale those were four wildly different boxes and
  // the 1h one had to drop its note to fit. The duration read moved to the axis
  // (see the next test); what a card gets is room.
  await expect(page.locator('.dc-blk')).toHaveCount(4);
  const boxes = await page.locator('.dc-blk').evaluateAll((els) =>
    els.map((el) => {
      const r = el.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, height: r.height };
    }),
  );

  for (const b of boxes) {
    expect(Math.abs(b.height - boxes[0].height)).toBeLessThan(1);
    // Big enough for a photograph, a name, a time and a note — which is the
    // whole reason the height stopped being a function of the clock.
    expect(b.height).toBeGreaterThan(120);
  }

  const gaps = boxes.slice(1).map((b, i) => b.top - boxes[i].bottom);
  for (const g of gaps) expect(Math.abs(g - gaps[0])).toBeLessThan(1);
  expect(gaps[0]).toBeGreaterThan(20);
});

test('the clock absorbs the duration the cards no longer carry', async ({ page }) => {
  await page.goto(DAY6);

  // The axis is piecewise: linear inside a row, a different scale from row to
  // row. So a card's top edge is its arrival and its bottom edge is its
  // departure, and the hour labels in the gutter land wherever that puts them.
  // Fushimi is 07:15–09:45, so 08:00 and 09:00 both fall inside it.
  const fushimi = (await page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' }).first().boundingBox())!;
  const hour = async (label: string) => (await page.locator('.dc-hour i', { hasText: label }).boundingBox())!;
  const eight = await hour('08:00');
  const nine = await hour('09:00');

  // Both inside the card's extent...
  const mid = (b: { y: number; height: number }) => b.y + b.height / 2;
  expect(mid(eight)).toBeGreaterThan(fushimi.y);
  expect(mid(nine)).toBeLessThan(fushimi.y + fushimi.height);

  // ...and an hour of a 2h30 stop is exactly 1/2.5 of that stop's height, so
  // the two labels are a predictable distance apart. This is the claim the
  // card heights used to make.
  const perHour = fushimi.height / 2.5;
  expect(Math.abs(mid(nine) - mid(eight) - perHour)).toBeLessThan(3);

  // Two labels closer together than a line of type would be one smudge, so the
  // axis drops the second rather than printing both.
  const ys = await page.locator('.dc-hour i').evaluateAll((els) => els.map((el) => el.getBoundingClientRect().top));
  for (let i = 1; i < ys.length; i += 1) expect(ys[i] - ys[i - 1]).toBeGreaterThan(20);
});

test('the hour labels at both ends of the day stay inside the rail', async ({ page }) => {
  // Day 1's window runs 14:00–22:00, so the first and last hour marks land on
  // the canvas's own edges — where `top: -0.5em` hung half of `14:00` out over
  // the page above the sky it belongs to.
  await page.goto(`${TRIP}/plan?view=timeline&day=d1`);
  const canvas = (await page.locator('.daycanvas').boundingBox())!;
  const labels = page.locator('.dc-hour i');
  await expect(labels.first()).toHaveText('14:00');
  await expect(labels.last()).toHaveText('22:00');

  for (const box of [(await labels.first().boundingBox())!, (await labels.last().boundingBox())!]) {
    expect(box.y).toBeGreaterThanOrEqual(canvas.y - 0.5);
    expect(box.y + box.height).toBeLessThanOrEqual(canvas.y + canvas.height + 0.5);
  }
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

test('free time at the head of a day is drawn too, and offers to fill itself', async ({ page }) => {
  // Day 7's window opens at 08:30 for a first stop at 09:45. Drawing only the
  // trailing gap said those seventy-five minutes were spoken for.
  await page.goto(`${TRIP}/plan?view=timeline&day=d7`);
  const lead = page.locator('.dc-tail.lead');
  await expect(lead).toBeVisible();
  await expect(lead).toContainText('1 h 15 unplanned');

  const canvas = (await page.locator('.daycanvas').boundingBox())!;
  const first = (await page.locator('.dc-blk').first().boundingBox())!;
  const box = (await lead.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(canvas.y - 1);
  expect(box.y + box.height).toBeLessThanOrEqual(first.y + 1);

  // The empty space and the control that fills it are the same object.
  await lead.click();
  await expect(page.getByRole('dialog')).toContainText('Propose a stop');
});

test('the two horizons are drawn as sun and moon in the gutter', async ({ page }) => {
  // Replaces `sunset 16:35 ☀ ↓` — three symbols doing one job, one of them the
  // sun announcing the end of the sun — printed on top of the itinerary.
  await page.goto(DAY6);
  const sunset = page.locator('.dc-sunset');
  await expect(sunset).toBeVisible();
  await expect(sunset.getByRole('img', { name: /^sunset \d\d:\d\d$/ })).toBeVisible();
  await expect(sunset).toContainText(/^\d\d:\d\d$/);

  // Out in the gutter, left of the column, so it can never land on a stop.
  const canvas = (await page.locator('.daycanvas').boundingBox())!;
  const mark = (await sunset.locator('.dc-hz-mark').boundingBox())!;
  expect(mark.x + mark.width).toBeLessThanOrEqual(canvas.x);

  // Day 6 opens at 07:00, after a 06:30 sunrise — so there is no sunrise to
  // mark, and inventing one would be worse than showing none.
  await expect(page.locator('.dc-sunrise')).toHaveCount(0);
});

test('a stop that straddles sunset darkens from the moment it does', async ({ page }) => {
  await page.goto(DAY6);
  // Arashiyama: 14:45 + 2h30 against a 16:51 sunset, so the sun goes down 84%
  // of the way through the visit. The card says so; a rule drawn across the
  // card would only have said so on top of the card's own note.
  const grove = page.locator('.dc-blk').filter({ hasText: 'Bamboo Grove' }).first();
  const duskAt = await grove.evaluate((el) => getComputedStyle(el).getPropertyValue('--dusk-at').trim());
  const pct = parseFloat(duskAt);
  expect(pct).toBeGreaterThan(70);
  expect(pct).toBeLessThan(95);

  // A stop that is wholly after dark starts dark at its top edge.
  await page.goto(`${TRIP}/plan?view=timeline&day=d7`);
  const kix = page.locator('.dc-blk').filter({ hasText: 'Kansai International' }).first();
  await expect(kix).toHaveClass(/after-dark/);
  expect(parseFloat(await kix.evaluate((el) => getComputedStyle(el).getPropertyValue('--dusk-at')))).toBe(0);
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

test('the canvas is sized by the day, not by the screen', async ({ page }) => {
  // The temptation is to compress the column until it fits the viewport, at
  // which point every card is a strip again. The height is a function of how
  // much is in the day and nothing else: day 6 has four stops, day 1 has three,
  // and the difference is exactly one card plus one gap.
  await page.goto(DAY6);
  const six = (await page.locator('.daycanvas').boundingBox())!;
  const card = (await page.locator('.dc-blk').first().boundingBox())!;
  const gap = (await page.locator('.dc-blk').nth(1).boundingBox())!.y - (card.y + card.height);

  await page.goto(`${TRIP}/plan?view=timeline&day=d1`);
  const one = (await page.locator('.daycanvas').boundingBox())!;
  await expect(page.locator('.dc-blk')).toHaveCount(3);

  expect(Math.abs(six.height - one.height - (card.height + gap))).toBeLessThan(2);
});
