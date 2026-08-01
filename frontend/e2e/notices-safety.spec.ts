import { expect, test } from '@playwright/test';

const PREP = '/trips/t-japan26/prep';
const TITLE = 'IC-card top-ups & cash — how much to carry';

test('notice management identifies its author and makes archive reversible', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(PREP);

  let card = page.locator('.notice', { hasText: TITLE });
  await expect(card).toBeVisible();
  await expect(card.getByText('By Ann')).toBeVisible();

  await card.getByRole('button', { name: 'Notice actions' }).click();
  await page.getByRole('menuitem', { name: /Copy source link/ }).click();
  await expect(page.getByRole('status')).toHaveText(/Source link copied/);

  await card.getByRole('button', { name: 'Notice actions' }).click();
  await page.getByRole('menuitem', { name: /^🗄️ Archive$/ }).click();

  const dialog = page.getByRole('alertdialog', { name: `Archive “${TITLE}”?` });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('It isn’t deleted');
  await page.waitForTimeout(300); // let the mobile bottom-sheet entrance finish before measuring it
  const box = await dialog.boundingBox();
  const viewport = page.viewportSize();
  expect(box).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(viewport!.width);
  expect(box!.y + box!.height).toBeLessThanOrEqual(viewport!.height);

  await dialog.getByRole('button', { name: 'Archive notice' }).click();
  await expect(card).toHaveCount(0);

  const archivedToggle = page.getByRole('button', { name: 'Show archived (1)' });
  await expect(archivedToggle).toBeVisible();
  await archivedToggle.click();
  card = page.locator('.notice.archived', { hasText: TITLE });
  await expect(card).toBeVisible();
  await expect(card).toContainText('archived');

  await card.getByRole('button', { name: 'Notice actions' }).click();
  await page.getByRole('menuitem', { name: /Restore to active/ }).click();
  await expect(page.locator('.notice:not(.archived)', { hasText: TITLE })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Show archived (1)' })).toHaveCount(0);
});
