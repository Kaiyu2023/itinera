import { expect, test } from '@playwright/test';

const TRIP = '/trips/t-japan26/candidates';

test('a member can create a manual idea with an optional, expandable activity pool', async ({ page }) => {
  await page.goto(TRIP);
  await page.getByRole('button', { name: '＋ Add an idea' }).click();

  const dialog = page.getByRole('dialog', { name: 'Add a trip idea' });
  await dialog.getByRole('button', { name: 'Enter manually' }).click();
  await dialog.getByLabel('Name').fill('Kiyosumi Gardens');
  await dialog.getByLabel('Type').selectOption('sight');
  await dialog.getByLabel('City').fill('Tokyo');
  await dialog.getByLabel('Address').fill('3-3-9 Kiyosumi, Koto City');
  await dialog.getByLabel('Website').fill('https://example.com/kiyosumi');
  await dialog.getByLabel('Phone').fill('+81 3 3641 5892');
  await dialog.getByLabel('Opening hours').fill('09:00–17:00\nClosed 29 Dec–1 Jan');
  await dialog.getByLabel('Photo URLs').fill('/photos/meiji-jingu-torii.webp');
  await dialog.getByLabel('Summary').fill('A calm landscape garden with stepping stones and a large pond.');
  await dialog
    .getByLabel('Introduction')
    .fill('This compact Meiji-era garden offers a slower alternative to Tokyo’s busier headline sights.');
  await dialog.getByLabel('Activity 1', { exact: true }).fill('Walk the pond circuit');
  await dialog
    .getByLabel('Optional details for activity 1')
    .fill('Allow about 35 minutes and look for turtles near the stone bridges.');
  await dialog.getByRole('button', { name: '＋ Add an activity' }).click();
  await dialog.getByLabel('Activity 2', { exact: true }).fill('Pause at the tea pavilion');
  // Details deliberately stay empty: the resulting card must not draw a
  // disclosure control for a self-explanatory activity.
  await dialog
    .getByLabel('Good to know')
    .fill('Last entry is 30 minutes before closing.\nBring mosquito repellent in summer.');
  await dialog.getByLabel('Why suggest this place').fill('A quiet reset between Tsukiji and the evening plans.');
  await dialog.getByLabel('Add a tag').fill('slow-day');
  await dialog.getByLabel('Add a tag').press('Enter');
  await dialog.getByRole('button', { name: 'Add to trip ideas' }).click();
  await expect(dialog).not.toBeVisible();

  const card = page.locator('.cand-card').filter({ hasText: 'Kiyosumi Gardens' });
  await expect(card).toContainText('A calm landscape garden');
  await expect(card).toContainText('slow-day');
  await card.getByText('Explore place guide', { exact: true }).click();
  await expect(card).toContainText('This compact Meiji-era garden');
  await expect(card).toContainText('3-3-9 Kiyosumi, Koto City');

  const detailedActivity = card.getByRole('listitem').filter({ hasText: 'Walk the pond circuit' });
  const simpleActivity = card.getByRole('listitem').filter({ hasText: 'Pause at the tea pavilion' });
  await detailedActivity.getByRole('button', { name: 'Show details for Walk the pond circuit' }).click();
  await expect(detailedActivity).toContainText('Allow about 35 minutes');
  await expect(simpleActivity.getByRole('button')).toHaveCount(0);
});

test('editing an idea preserves authored text, optional details, and catalog search isolation', async ({ page }) => {
  await page.goto(TRIP);
  const card = page.locator('.cand-card').filter({ hasText: 'Ghibli Museum' });
  await card.getByRole('button', { name: 'Edit idea' }).click();

  let dialog = page.getByRole('dialog', { name: 'Edit trip idea' });
  await expect(dialog.getByLabel('Name')).toHaveValue('Ghibli Museum');
  await dialog.getByLabel('Summary').fill('A playful museum where animation craft becomes an explorable place.');
  await dialog.getByLabel('Optional details for activity 1').fill('');
  await dialog.getByLabel('Why suggest this place').fill('Kaiyu wants the group to enter the ticket lottery together.');
  await dialog.getByLabel('Good to know').fill('Tickets are timed and must be bought in advance.');
  await dialog.getByLabel('Add a tag').fill('edited-by-kaiyu');
  await dialog.getByLabel('Add a tag').press('Enter');
  await dialog.getByRole('button', { name: 'Save changes' }).click();
  await expect(dialog).not.toBeVisible();

  await expect(card).toContainText('A playful museum where animation craft becomes an explorable place.');
  await expect(card).toContainText('Kaiyu wants the group');
  await expect(card).toContainText('edited-by-kaiyu');
  await card.getByText('Explore place guide', { exact: true }).click();
  const activity = card.getByRole('listitem').filter({ hasText: 'Explore the animation exhibits' });
  await expect(activity.getByRole('button')).toHaveCount(0);

  // A second save creates another immutable snapshot, but neither snapshot is
  // allowed back into search results as a duplicate catalog place.
  await card.getByRole('button', { name: 'Edit idea' }).click();
  dialog = page.getByRole('dialog', { name: 'Edit trip idea' });
  await dialog.getByRole('button', { name: 'Save changes' }).click();
  await page.getByRole('button', { name: '＋ Add an idea' }).click();
  const addDialog = page.getByRole('dialog', { name: 'Add a trip idea' });
  await addDialog.getByLabel('Search places').fill('Ghibli Museum');
  await expect(addDialog.locator('.place-results').getByRole('button', { name: /Ghibli Museum/ })).toHaveCount(1);
  await addDialog.getByRole('button', { name: 'Close' }).click();

  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();
  await expect(page.getByRole('button', { name: '编辑灵感' }).first()).toBeVisible();
  await expect(card).toContainText('A playful museum where animation craft becomes an explorable place.');

  // Adopted ideas intentionally have no place editor: changing their candidate
  // snapshot must not masquerade as an edit to the already-applied plan stop.
  const adopted = page.locator('.cand-card').filter({ hasText: 'teamLab Planets' });
  await expect(adopted.getByRole('button', { name: '编辑灵感' })).toHaveCount(0);
});
