import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

const TRIP = '/trips/t-japan26';
const addStopComposer = (page: Page) => page.locator('.compose.compose-hasmap, .compose.compose-docked');

test('a search hit in a new city keeps that city in an editable combobox', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=addStop&day=d5&mode=new&q=Nara%20Park&pick=first`);

  const composer = addStopComposer(page);
  await expect(composer).toBeVisible();
  await expect(composer.getByText('Nara Park', { exact: true }).first()).toBeVisible();

  const city = composer.getByRole('combobox', { name: 'City *' });
  await expect(city).toHaveValue('Nara');
  await expect(city).toBeEditable();
  await expect(city).toHaveAttribute('list', /-suggestions$/);
  expect(
    await city.evaluate((input: HTMLInputElement) => [...(input.list?.options ?? [])].map((o) => o.value)),
  ).toContain('Nara');

  // Cities from the trip are suggestions, not an allow-list. A genuinely new
  // value remains in the draft and in the exact operation shown for review.
  await city.fill('Uji');
  await expect(composer.locator('.preview')).toContainText('new · Uji');
  await expect(composer.getByRole('button', { name: 'Open the poll →' })).toBeEnabled();

  await city.fill('');
  await expect(composer.getByRole('button', { name: 'Open the poll →' })).toBeDisabled();

  const bounds = await city.evaluate((input) => {
    const control = input.getBoundingClientRect();
    const surface = input.closest('.compose')!.getBoundingClientRect();
    return {
      controlLeft: control.left,
      controlRight: control.right,
      surfaceLeft: surface.left,
      surfaceRight: surface.right,
    };
  });
  expect(bounds.controlLeft).toBeGreaterThanOrEqual(bounds.surfaceLeft);
  expect(bounds.controlRight).toBeLessThanOrEqual(bounds.surfaceRight);
});

test('reusing a trip place shows saved details instead of editable fields', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=addStop&day=d2&mode=new&q=Meiji&pick=first`);

  const composer = addStopComposer(page);
  const reuse = composer.getByRole('note');
  await expect(reuse).toContainText('Reusing Meiji Jingū');
  await expect(reuse).toContainText('references the trip’s saved place as-is');
  await expect(reuse).toContainText('Sight · Tokyo');

  // The add_stop operation references the existing place. Hiding the draft
  // controls prevents the UI from promising edits that operation cannot save.
  await expect(composer.getByPlaceholder('e.g. Kissa Master (kissaten)')).toHaveCount(0);
  await expect(composer.getByRole('combobox', { name: 'City *' })).toHaveCount(0);
  await expect(composer.getByPlaceholder('Google Maps or website (optional)')).toHaveCount(0);
  await expect(composer.getByPlaceholder('Anything the group should know (optional)')).toHaveCount(0);
  await expect(composer.locator('.preview')).toContainText('Meiji Jingū');

  // Clearing the catalog selection deliberately switches back to a new-place
  // draft, where the prefilled details are honestly editable again.
  await composer.getByRole('button', { name: '✕ Clear selection · enter by hand' }).click();
  await expect(composer.getByPlaceholder('e.g. Kissa Master (kissaten)')).toHaveValue('Meiji Jingū');
  await expect(composer.getByRole('combobox', { name: 'City *' })).toHaveValue('Tokyo');
  await expect(composer.getByPlaceholder('Google Maps or website (optional)')).toBeEditable();
});

test('the reuse explanation is localized without translating the place name', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('itinera.locale', 'zh-CN'));
  await page.goto(`${TRIP}/plan?gov=addStop&day=d2&mode=new&q=Meiji&pick=first`);

  const reuse = addStopComposer(page).getByRole('note');
  await expect(reuse).toContainText('复用 Meiji Jingū');
  await expect(reuse).toContainText('名称、类型、城市、链接和备注不能在这里更改');
  await expect(reuse).toContainText('景点 · Tokyo');
});
