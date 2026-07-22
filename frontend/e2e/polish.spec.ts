import { test, expect } from '@playwright/test';

/** Coverage for the polish round — including the paths that would have caught
    the route bug: the proposal route toggle end-to-end, discussions, foldable
    candidates, candidate→plan, notice audiences, tinting, and the credit link. */

const TRIP = '/trips/t-japan26';

test('change composer defaults to poll and the button follows the route', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=change&stop=s-d1-hotel`);
  const pollSeg = page.getByRole('button', { name: 'Open a poll' });
  await expect(pollSeg).toHaveClass(/active/);
  await expect(page.getByRole('button', { name: 'Open the poll →' })).toBeVisible();
  await page.getByRole('button', { name: "Request a leader's approval" }).click();
  await expect(page.getByRole('button', { name: 'Send to leaders →' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open the poll →' })).toHaveCount(0);
});

test('a poll-routed proposal opens a live poll', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=change&stop=s-d1-hotel`);
  // Move to a different day so the change is a real op.
  await page.locator('.compose select').first().selectOption('d2');
  await page.getByRole('button', { name: 'Open the poll →' }).click();
  await expect(page.getByText('Poll opened ✓')).toBeVisible();
  await page.getByRole('button', { name: 'Done' }).click();
  // Client-side navigation — a full reload would reset the in-memory mock.
  await page.getByRole('link', { name: 'Polls' }).filter({ visible: true }).first().click();
  // Exactly once: as the open poll — a poll-wrapped proposal must NOT also
  // sit in the "Awaiting a decision" leader queue (racing decision paths).
  const title = page.getByText('Move Hotel Gracery Shinjuku to Day 2');
  await expect(title).toHaveCount(1);
  await expect(title).toBeVisible();
});

test('start a discussion on a stop without a thread', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=discuss&stop=s-d1-omoide`);
  await expect(page.getByText(/No discussion on this stop yet/)).toBeVisible();
  await page.getByPlaceholder('Start the discussion…').fill('Two groups of three if the stalls are packed?');
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.getByText('Two groups of three if the stalls are packed?')).toBeVisible();
  await expect(page.getByPlaceholder('Add to the thread…')).toBeVisible();
});

test('comment on an existing thread', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=discuss&stop=s-d4-ryokan`);
  await expect(page.getByText('Onsen etiquette + the tattoo question')).toBeVisible();
  await page.getByPlaceholder('Add to the thread…').fill('Booked the riverside bath for 21:00.');
  await page.keyboard.press('Enter');
  await expect(page.getByText('Booked the riverside bath for 21:00.')).toBeVisible();
});

test('candidate sections fold and unfold', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  // Collapse is a 0fr grid animation — content is clipped, not display:none,
  // so assert on the body's real height instead of Playwright visibility.
  const votedOff = page.getByRole('button', { name: /Voted off/ });
  await expect(votedOff).toHaveAttribute('aria-expanded', 'false');
  const votedBody = page.locator('.cand-section', { has: votedOff }).locator('.cand-section-body');
  expect(((await votedBody.boundingBox())?.height ?? 99)).toBeLessThan(8);
  await votedOff.click();
  await expect(votedOff).toHaveAttribute('aria-expanded', 'true');
  await expect.poll(async () => (await votedBody.boundingBox())?.height ?? 0).toBeGreaterThan(50);
  const competing = page.getByRole('button', { name: /Competing for a slot/ });
  await expect(competing).toHaveAttribute('aria-expanded', 'true');
  const competingBody = page.locator('.cand-section', { has: competing }).locator('.cand-section-body');
  await competing.click();
  await expect(competing).toHaveAttribute('aria-expanded', 'false');
  await expect.poll(async () => (await competingBody.boundingBox())?.height ?? 0).toBeLessThan(8);
});

test('propose a shortlisted candidate for the plan', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  await page.getByRole('button', { name: /Propose for the plan/ }).first().click();
  await expect(page).toHaveURL(/\/plan/);
  // Composer opens in candidates mode with the candidate preselected and a day picker.
  await expect(page.getByText('Ghibli Museum').first()).toBeVisible();
  await page.getByRole('button', { name: 'Open the poll →' }).click();
  await expect(page.getByText('Poll opened ✓')).toBeVisible();
});

test('notice composer scopes the audience', async ({ page }) => {
  await page.goto(`${TRIP}/prep?prep=new`);
  const dialog = page.getByRole('dialog', { name: 'New notice' });
  await expect(dialog).toBeVisible();
  const chips = dialog.locator('.aud-chip');
  await expect(chips.first()).toHaveAttribute('aria-pressed', 'true');
  // Trim the audience down to a subset.
  await chips.nth(2).click();
  await expect(chips.nth(2)).toHaveAttribute('aria-pressed', 'false');
  await dialog.getByPlaceholder('Short, plain headline').fill('Rail passes for the Hakone leg');
  await dialog.getByPlaceholder(/Markdown ok/).fill('Only the Hakone hikers need the Free Pass.');
  await dialog.getByRole('button', { name: 'Post notice' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByText('Rail passes for the Hakone leg')).toBeVisible();
});

test('subset-audience notice shows who it is for', async ({ page }) => {
  await page.goto(`${TRIP}/prep`);
  await expect(page.getByText(/For Makoto & Kaiyu/)).toBeVisible();
});

test('header credits the author with a GitHub link', async ({ page, isMobile }) => {
  await page.goto('/');
  // The tagline slot is CSS-hidden on small screens, so query by href, not role.
  const credit = page.locator('a[href="https://github.com/Kaiyu2023/itinera"]');
  await expect(credit).toHaveText('By Kaiyu2023');
  if (!isMobile) await expect(credit).toBeVisible();
});

test('trip pages tint the background with the trip accent', async ({ page }) => {
  await page.goto(`${TRIP}/plan`);
  await expect(page.locator('body')).toHaveClass(/trip-tinted/);
  await page.goto('/');
  await expect(page.locator('body')).not.toHaveClass(/trip-tinted/);
});
