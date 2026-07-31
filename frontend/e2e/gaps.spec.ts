import { test, expect } from '@playwright/test';

/** The four flows added in the gaps round: pitch a candidate, create a trip,
    open a standalone poll, and inline content edits on stops/days. */

const TRIP = '/trips/t-japan26';

test('pitch an idea: search, select, and add a candidate', async ({ page }) => {
  await page.goto(`${TRIP}/candidates?cand=new&q=Shibuya%20Sky&pick=first`);
  const dialog = page.getByRole('dialog', { name: 'Pitch an idea' });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator('.cand-picked')).toContainText('Shibuya Sky');
  await dialog.getByLabel('Pitch').fill('Sunset over the crossing — best skyline deck in Tokyo.');
  await dialog.getByLabel('Add a tag').fill('views');
  await dialog.getByLabel('Add a tag').press('Enter');
  await dialog.getByRole('button', { name: 'Add to shortlist' }).click();
  await expect(dialog).not.toBeVisible();
  const card = page.locator('.cand-card', { hasText: 'Shibuya Sky' });
  await expect(card).toBeVisible();
  await expect(card).toContainText('best skyline deck');
  await expect(card).toContainText('views');
});

test('create a trip and land on its plan', async ({ page }) => {
  await page.goto('/?trip=new');
  const dialog = page.getByRole('dialog', { name: 'New trip' });
  await expect(dialog).toBeVisible();
  await dialog.locator('#trip-name').fill('Seoul, Cherry Blossom');
  await dialog.getByLabel('Start date').fill('2027-04-02');
  await dialog.getByLabel('End date').fill('2027-04-08');
  await dialog.getByRole('button', { name: 'Create trip' }).click();
  await expect(page).toHaveURL(/\/trips\/t-/);
  await expect(page.getByRole('heading', { name: 'Seoul, Cherry Blossom' })).toBeVisible();
});

test('create-trip form rejects end before start', async ({ page }) => {
  await page.goto('/?trip=new');
  const dialog = page.getByRole('dialog', { name: 'New trip' });
  await dialog.locator('#trip-name').fill('Backwards');
  await dialog.getByLabel('Start date').fill('2027-04-08');
  await dialog.getByLabel('End date').fill('2027-04-02');
  await expect(dialog.getByRole('button', { name: 'Create trip' })).toBeDisabled();
});

test('standalone poll opens live and takes votes', async ({ page }) => {
  await page.goto(`${TRIP}/polls?poll=new`);
  const dialog = page.getByRole('dialog', { name: 'New poll' });
  await expect(dialog).toBeVisible();
  await dialog.locator('#poll-q').fill('Karaoke night: which evening?');
  await dialog.getByRole('textbox', { name: 'Option 1' }).fill('Tokyo, Nov 15');
  await dialog.getByRole('textbox', { name: 'Option 2' }).fill('Osaka, Nov 20');
  await dialog.getByRole('button', { name: 'Open poll' }).click();
  await expect(dialog).not.toBeVisible();
  const poll = page.locator('.card', { hasText: 'Karaoke night' }).first();
  await expect(poll).toBeVisible();
  // A single-choice option on an open poll is a radio, not a button — it has a
  // checked state and it is one of a set. Closed polls drop the role, because
  // there is nothing left to choose.
  await poll.getByRole('radio', { name: /Tokyo, Nov 15/ }).click();
  await expect(poll.getByText('· your vote')).toBeVisible();
});

test('inline stop edit saves without governance', async ({ page }) => {
  await page.goto(`${TRIP}/plan?view=timeline&edit=stop:s-d2-gyozalou`);
  const dialog = page.getByRole('dialog', { name: /^Edit / });
  await expect(dialog).toBeVisible();
  await dialog.locator('#stop-notes').fill('Cash only — bring ¥5000. Queue moves fast.');
  await dialog.getByRole('button', { name: 'Save changes' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByText('bring ¥5000')).toBeVisible();
  // Content edits bypass proposals: no new pending proposal should appear.
  await expect(page.getByText(/proposal pending/i)).toHaveCount(0);
});

test('inline day edit saves window and city', async ({ page }) => {
  await page.goto(`${TRIP}/plan?view=timeline&edit=day:d5`);
  const dialog = page.getByRole('dialog', { name: 'Edit Day 5' });
  await expect(dialog).toBeVisible();
  await dialog.locator('#day-city').fill('Kyoto (Gion)');
  await dialog.getByLabel('Window start').fill('08:30');
  await dialog.getByRole('button', { name: 'Save changes' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByText('Kyoto (Gion)').first()).toBeVisible();
});
