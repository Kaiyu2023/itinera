import { test, expect } from '@playwright/test';

/** The timeline is a truthful clock first, with detail disclosed on selection. */

const TRIP = '/trips/t-japan26';
const DAY6 = `${TRIP}/plan?view=timeline&day=d6`;

test('stop height is proportional to duration everywhere on the clock', async ({ page }) => {
  await page.goto(DAY6);

  await expect(page.locator('.dc-blk')).toHaveCount(4);
  const cards = await page.locator('.dc-blk').evaluateAll((els) =>
    els.map((el) => {
      const r = el.getBoundingClientRect();
      const text = el.textContent ?? '';
      const duration =
        text.includes('Fushimi Inari') || text.includes('Bamboo Grove') ? 150 : text.includes('Kiyomizu') ? 100 : 60;
      return { text, height: r.height, duration };
    }),
  );

  const pixelsPerMinute = cards.map((card) => card.height / card.duration);
  for (const scale of pixelsPerMinute) expect(scale).toBeCloseTo(1.6, 1);
  expect(cards.find((card) => card.text.includes('Fushimi Inari'))!.height).toBeCloseTo(240, 0);
  expect(cards.find((card) => card.text.includes('Yoshimura'))!.height).toBeCloseTo(96, 0);
});

test('consecutive hour marks are evenly spaced and none are omitted', async ({ page }) => {
  await page.goto(DAY6);

  const fushimi = (await page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' }).first().boundingBox())!;
  const hour = async (label: string) => (await page.locator('.dc-hour i', { hasText: label }).boundingBox())!;
  const eight = await hour('08:00');
  const nine = await hour('09:00');

  const mid = (b: { y: number; height: number }) => b.y + b.height / 2;
  expect(mid(eight)).toBeGreaterThan(fushimi.y);
  expect(mid(nine)).toBeLessThan(fushimi.y + fushimi.height);
  expect(mid(nine) - mid(eight)).toBeCloseTo(96, 0);

  const marks = await page
    .locator('.dc-hour')
    .evaluateAll((els) => els.map((el) => ({ text: el.textContent, y: el.getBoundingClientRect().top })));
  expect(marks.map((mark) => mark.text)).toEqual([
    '07:00',
    '08:00',
    '09:00',
    '10:00',
    '11:00',
    '12:00',
    '13:00',
    '14:00',
    '15:00',
    '16:00',
    '17:00',
    '18:00',
  ]);
  for (let i = 1; i < marks.length; i += 1) expect(marks[i].y - marks[i - 1].y).toBeCloseTo(96, 0);
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
    expect(box.y + box.height).toBeLessThanOrEqual(canvas.y + canvas.height + 1);
  }
});

test('a stop that runs past sunset keeps a semantic, subtle marker', async ({ page }) => {
  await page.goto(DAY6);

  // The fixture note says "the 14:45 Arashiyama arrival leaves little daylight
  // for the grove". That should be something you can see, not something you
  // have to read.
  const grove = page.locator('.dc-blk').filter({ hasText: 'Bamboo Grove' }).first();
  await expect(grove).toHaveClass(/after-dark/);
  await expect(grove.getByText(/sunset 16:51/)).toBeVisible();

  // ...and it must genuinely sit below the sunset rule.
  const sunset = page.locator('.dc-sunset');
  await expect(sunset.locator('.dc-hz-mark')).toBeVisible();
  const groveBox = (await grove.boundingBox())!;
  const sunsetY = await sunset.evaluate((el) => el.getBoundingClientRect().top);
  expect(groveBox.y + groveBox.height).toBeGreaterThan(sunsetY);

  // An earlier stop on the same day must not be flagged.
  await expect(page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' }).first()).not.toHaveClass(/after-dark/);
});

test('unused window time is shown as space, not as a percentage', async ({ page }) => {
  await page.goto(DAY6);
  // Day 6 uses 575 of 660 minutes. The remainder is drawn.
  await expect(page.locator('.dc-tail')).toBeVisible();
  await expect(page.locator('.dc-tail')).toContainText('free');
});

test('free time at the head of a day is drawn too, and offers to fill itself', async ({ page }) => {
  // Day 7's window opens at 08:30 for a first stop at 09:45. Drawing only the
  // trailing gap said those seventy-five minutes were spoken for.
  await page.goto(`${TRIP}/plan?view=timeline&day=d7`);
  const lead = page.locator('.dc-tail.lead');
  await expect(lead).toBeVisible();
  await expect(lead).toContainText('1 h 15 free');

  const canvas = (await page.locator('.daycanvas').boundingBox())!;
  const first = (await page.locator('.dc-blk').first().boundingBox())!;
  const box = (await lead.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(canvas.y - 1);
  expect(box.y + box.height).toBeLessThanOrEqual(first.y + 1);

  // The empty space and the control that fills it are the same object.
  await lead.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Propose a stop');
  await expect(dialog.locator('.field', { hasText: 'Insert' }).locator('select')).toHaveValue('first');
});

test('the two horizons are drawn as compact markers in the gutter', async ({ page }) => {
  // Replaces `sunset 16:35 ☀ ↓` — three symbols doing one job, one of them the
  // sun announcing the end of the sun — printed on top of the itinerary.
  await page.goto(DAY6);
  const sunset = page.locator('.dc-sunset');
  const horizonMark = sunset.locator('.dc-hz-mark');
  await expect(horizonMark).toBeVisible();
  // Shape is the redundant encoding when colour is unavailable. It remains
  // visible in the compact phone rail instead of leaving a purple time-only
  // chip whose meaning depends on hue.
  await expect(sunset.getByRole('img', { name: /^sunset \d\d:\d\d$/ })).toBeVisible();
  await expect(horizonMark).toContainText(/^\d\d:\d\d$/);

  // Out in the gutter, left of the column, so it can never land on a stop.
  const canvas = (await page.locator('.daycanvas').boundingBox())!;
  const mark = (await horizonMark.boundingBox())!;
  expect(mark.x + mark.width).toBeLessThanOrEqual(canvas.x);

  // Day 6 opens at 07:00, after a 06:30 sunrise — so there is no sunrise to
  // mark, and inventing one would be worse than showing none.
  await expect(page.locator('.dc-sunrise')).toHaveCount(0);
});

test('sunset is annotated without painting a dusk overlay over cards', async ({ page }) => {
  await page.goto(DAY6);
  const grove = page.locator('.dc-blk').filter({ hasText: 'Bamboo Grove' }).first();
  await expect(grove).toContainText('sunset 16:51');
  expect(await grove.evaluate((el) => getComputedStyle(el, '::before').display)).toBe('none');

  await page.goto(`${TRIP}/plan?view=timeline&day=d7`);
  const kix = page.locator('.dc-blk').filter({ hasText: 'Kansai International' }).first();
  await expect(kix).toHaveClass(/after-dark/);
  await expect(kix).toContainText('after dark');
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
  await expect(page.getByRole('heading', { name: 'Hakone', exact: true })).toBeVisible();
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

test('the canvas height is determined by its time window', async ({ page }) => {
  await page.goto(DAY6);
  const six = (await page.locator('.daycanvas').boundingBox())!;

  await page.goto(`${TRIP}/plan?view=timeline&day=d1`);
  const one = (await page.locator('.daycanvas').boundingBox())!;
  await expect(page.locator('.dc-blk')).toHaveCount(3);

  // Day 6 is 07:00–18:00 (11h), Day 1 is 14:00–22:00 (8h).
  expect(six.height - one.height).toBeCloseTo(3 * 96, 0);
});

test('the selected-stop inspector stays below the day tabs while browsing later hours', async ({ page, isMobile }) => {
  test.skip(isMobile, 'desktop selected-stop inspector');
  await page.goto(`${TRIP}/plan?view=timeline&day=d2`);

  const inspector = page.locator('.timeline-inspector');
  const dayTabs = page.getByRole('tablist', { name: 'Days' });
  await expect(inspector.getByRole('heading', { name: 'Meiji Jingū' })).toBeVisible();
  await expect(inspector).toHaveCSS('position', 'sticky');

  // Move well into the clock. This used to put the inspector under the day tabs.
  await page.locator('.daycanvas').evaluate((canvas) => {
    const box = canvas.getBoundingClientRect();
    window.scrollTo(0, window.scrollY + box.top + 420);
  });
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(0);

  await expect
    .poll(async () => {
      const tabs = (await dayTabs.boundingBox())!;
      const card = (await inspector.boundingBox())!;
      return card.y - (tabs.y + tabs.height);
    })
    .toBeGreaterThanOrEqual(7);

  const card = (await inspector.boundingBox())!;
  expect(card.y).toBeGreaterThanOrEqual(0);
  expect(card.y + card.height).toBeLessThanOrEqual(page.viewportSize()!.height + 1);
  await expect(inspector.getByRole('heading', { name: 'Meiji Jingū' })).toBeVisible();
});

test('the mobile selected-stop sheet is opt-in and explicitly dismissible', async ({ page, isMobile }) => {
  test.skip(!isMobile, 'mobile selected-stop sheet');
  await page.goto(`${TRIP}/plan?view=timeline&day=d2`);

  const inspector = page.locator('.timeline-inspector');
  const stop = page.locator('.daycanvas').getByRole('button', { name: /Meiji Jingū/ });
  await expect(inspector).not.toBeVisible();
  await expect(stop).toHaveAttribute('aria-pressed', 'false');
  await expect(stop).toHaveAttribute('aria-expanded', 'false');

  await stop.click();
  const sheet = page.getByRole('dialog', { name: 'Meiji Jingū' });
  await expect(sheet).toBeVisible();
  await expect(inspector.getByRole('heading', { name: 'Meiji Jingū' })).toHaveCount(0);
  await expect(stop).toHaveAttribute('aria-pressed', 'true');
  await expect(stop).toHaveAttribute('aria-expanded', 'true');

  const close = sheet.getByRole('button', { name: 'Close details for Meiji Jingū' });
  const closeBox = (await close.boundingBox())!;
  expect(closeBox.width).toBeGreaterThanOrEqual(44);
  expect(closeBox.height).toBeGreaterThanOrEqual(44);
  expect(closeBox.y).toBeGreaterThanOrEqual(0);
  expect(closeBox.y + closeBox.height).toBeLessThanOrEqual(page.viewportSize()!.height);
  const sheetBox = (await sheet.boundingBox())!;
  expect(sheetBox.y).toBeGreaterThanOrEqual(0);

  await close.click();
  await expect(sheet).not.toBeVisible();
  await expect(stop).toHaveAttribute('aria-pressed', 'false');
  await expect(stop).toHaveAttribute('aria-expanded', 'false');
  await expect(stop).toBeFocused();
});
