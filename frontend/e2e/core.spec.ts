import { test, expect } from '@playwright/test';

/** Core regression: every pre-existing flow the mock supports.
    The mock is in-memory, so each test asserts within one page session. */

const TRIP = '/trips/t-japan26';

test('trip list shows the trip and opens it', async ({ page }) => {
  await page.goto('/');
  const card = page.getByText('Japan, Autumn Leaves').first();
  await expect(card).toBeVisible();
  await card.click();
  await expect(page).toHaveURL(/\/trips\/t-japan26/);
  await expect(page.getByRole('heading', { name: 'Japan, Autumn Leaves' })).toBeVisible();
});

test('plan timeline renders all days with governance actions', async ({ page, isMobile }) => {
  await page.goto(`${TRIP}/plan?view=timeline`);
  await expect(page.getByText(/Plan v3 · 7 days/)).toBeVisible();
  // One day renders at a time; the scrubber holds all seven.
  const chips = page.getByRole('tablist', { name: 'Days' }).getByRole('tab');
  await expect(chips).toHaveCount(7);
  await chips.nth(3).click(); // Nov 17 — the Hakone day
  await expect(page.getByText('Hakone').first()).toBeVisible();
  // Phones start without a selected-stop surface so the clock remains clear.
  // Selecting a stop opens the detail sheet that owns these actions.
  if (isMobile) {
    await page.locator('.daycanvas .dc-blk-hit').first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
  }
  expect(await page.getByRole('button', { name: /Propose change/ }).count()).toBeGreaterThan(0);
  expect(await page.getByRole('button', { name: /Discuss/ }).count()).toBeGreaterThan(0);
});

test('add-stop deep link opens the composer with a search hit selected', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=addStop&day=d2&mode=new&q=Shibuya%20Sky&pick=first`);
  await expect(page.getByText(/Propose a stop · Day 2/)).toBeVisible();
  await expect(page.getByText('Shibuya Sky').first()).toBeVisible();
});

test('trip ideas tab renders each decision-state section', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  await expect(page.getByRole('heading', { name: 'Trip ideas' })).toBeVisible();
  await expect(page.getByText('Ideas to consider')).toBeVisible();
  await expect(page.getByText('Added to the plan')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Passed on' })).toBeVisible();
});

test('voting on an open poll registers my vote', async ({ page }) => {
  await page.goto(`${TRIP}/polls`);
  await expect(page.getByText(/quorum/).first()).toBeVisible();
  const option = page.locator('button:has(.fill)').first();
  await option.click();
  await expect(page.getByText('· your vote').first()).toBeVisible();
});

test('ledger shows balances and records a new expense', async ({ page }) => {
  await page.goto(`${TRIP}/ledger?ledger=add`);
  await page.getByLabel('Amount').fill('120');
  await page.getByPlaceholder('What was it for?').fill('Taxi to Haneda');
  await page.getByRole('button', { name: /^Add [$¥€£]/ }).click();
  await expect(page.getByText('Taxi to Haneda')).toBeVisible();
  // The "owes / is owed" axis key is desktop-only; balance rows show everywhere.
  await expect(page.locator('.bal-row').first()).toBeVisible();
});

test('prep checklist toggle updates my progress', async ({ page }) => {
  await page.goto(`${TRIP}/prep`);
  // The first undone item may sit under any notice — assert the global count.
  await expect(page.locator('.check-item').first()).toBeVisible();
  const before = await page.locator('.check-item.done').count();
  await page.locator('.check-item:not(.done)').first().click();
  await expect(page.locator('.check-item.done')).toHaveCount(before + 1);
});

test('review queue page renders', async ({ page }) => {
  await page.goto('/review');
  await expect(page.getByRole('heading', { name: 'Your review queue' })).toBeVisible();
  await expect(page.getByText('Trip · Japan, Autumn Leaves')).toHaveCount(3);
  await expect(page.getByRole('button', { name: 'Approve — publishes for a leader or a poll' })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Approve — applies now as your edit (via AI)' })).toHaveCount(2);
});

test('a11y shell: skip link and main landmark exist', async ({ page }) => {
  await page.goto(`${TRIP}/plan`);
  await expect(page.locator('a.skip-link')).toHaveAttribute('href', '#main');
  await expect(page.locator('main#main')).toBeVisible();
});
