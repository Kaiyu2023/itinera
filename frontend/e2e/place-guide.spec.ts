import { test, expect } from '@playwright/test';

const TRIP = '/trips/t-japan26';
const MEIJI_SUMMARY = 'A quiet forested Shinto shrine beside the bustle of Harajuku.';
const MEIJI_INTRO =
  "Meiji Jingu is reached by a long wooded approach that creates a calm transition from Harajuku to the shrine's main sanctuary.";
const GHIBLI_SUMMARY = "A small, imaginative museum devoted to Studio Ghibli's craft and worlds.";
const GHIBLI_INTRO =
  'The Ghibli Museum in Mitaka combines animation exhibits, playful architecture and spaces designed to be explored rather than rushed.';

test('the timeline keeps the guide summary concise and reveals the full guide in stop details', async ({
  page,
  isMobile,
}) => {
  await page.goto(`${TRIP}/plan?view=timeline&day=d2`);

  const stop = page.locator('.daycanvas').getByRole('button', { name: /Meiji Jingū/ });
  await expect(stop).toContainText(MEIJI_SUMMARY);
  await expect(stop).not.toContainText(MEIJI_INTRO);

  if (isMobile) await stop.click();
  const detail = isMobile ? page.getByRole('dialog', { name: 'Meiji Jingū' }) : page.locator('.timeline-inspector');
  await expect(detail).toBeVisible();

  // Primary/secondary hierarchy must survive both the desktop inspector and
  // the mobile sheet's tinted action footer.
  for (const actionName of ['Propose change', 'Discuss']) {
    const background = await detail
      .getByRole('button', { name: actionName, exact: true })
      .evaluate((element) => getComputedStyle(element).backgroundColor);
    expect(background).not.toBe('rgba(0, 0, 0, 0)');
  }

  const guide = detail.getByRole('region', { name: 'Guide to Meiji Jingū' });
  await expect(guide).toContainText(MEIJI_SUMMARY);
  await expect(guide).toContainText(MEIJI_INTRO);
  await expect(guide.getByRole('heading', { name: 'Ideas while you’re here' })).toBeVisible();
  await expect(guide).toContainText('Walk beneath the large torii gates');
  await expect(guide.getByText('Trip note', { exact: true })).toBeVisible();

  const walkDetails = guide
    .getByRole('listitem')
    .filter({ hasText: 'Walk beneath the large torii gates' })
    .getByRole('button');
  const sanctuaryDetails = guide
    .getByRole('listitem')
    .filter({ hasText: 'Visit the sanctuary and sake-barrel display' })
    .getByRole('button');
  await expect(walkDetails).toHaveAttribute('aria-expanded', 'false');
  await expect(sanctuaryDetails).toHaveAttribute('aria-expanded', 'false');
  await expect(guide.getByText(/broad gravel approach is part of the experience/)).not.toBeVisible();

  // A native button keeps the disclosure keyboard-operable, and each row owns
  // its state instead of opening the whole activity pool at once.
  await walkDetails.focus();
  await page.keyboard.press('Enter');
  await expect(walkDetails).toHaveAttribute('aria-expanded', 'true');
  await expect(guide.getByText(/broad gravel approach is part of the experience/)).toBeVisible();
  await expect(sanctuaryDetails).toHaveAttribute('aria-expanded', 'false');

  await expect(walkDetails).toHaveAccessibleName('Hide details for Walk beneath the large torii gates');
  await page.keyboard.press('Space');
  await expect(walkDetails).toHaveAttribute('aria-expanded', 'false');
  await expect(guide.getByText(/broad gravel approach is part of the experience/)).not.toBeVisible();
});

test('a trip idea discloses the long guide without burying its suggestion context', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);

  const card = page.locator('.cand-card').filter({ hasText: 'Ghibli Museum' });
  const guide = card.getByRole('region', { name: 'Guide to Ghibli Museum' });
  await expect(guide).toContainText(GHIBLI_SUMMARY);
  await expect(guide).toContainText('Why Ann suggested it');
  await expect(guide.getByText(GHIBLI_INTRO)).not.toBeVisible();

  await guide.getByText('Explore place guide', { exact: true }).click();
  await expect(guide.getByText(GHIBLI_INTRO)).toBeVisible();
  await expect(guide.getByRole('heading', { name: 'Ideas while you’re here' })).toBeVisible();
  await expect(guide).toContainText('Explore the animation exhibits');
  await expect(guide.getByRole('button', { name: 'Show details for Explore the animation exhibits' })).toBeVisible();
  await expect(
    guide.getByRole('button', { name: 'Show details for Watch the museum-only short film and visit the rooftop' }),
  ).toHaveCount(0);
  await expect(guide.getByText('Watch the museum-only short film and visit the rooftop')).toBeVisible();
});

test('a place without editorial guide copy still shows the trip idea and practical facts', async ({ page }) => {
  await page.goto(`${TRIP}/candidates?cand=new&q=Shibuya%20Sky&pick=first`);

  const dialog = page.getByRole('dialog', { name: 'Add a trip idea' });
  await dialog
    .getByLabel('Why suggest this place')
    .fill('Sunset over Shibuya would give the group a useful skyline option.');
  await dialog.getByRole('button', { name: 'Add to trip ideas' }).click();
  await expect(dialog).not.toBeVisible();

  const card = page.locator('.cand-card').filter({ hasText: 'Shibuya Sky' });
  const guide = card.getByRole('region', { name: 'Guide to Shibuya Sky' });
  await expect(guide).toContainText('Sunset over Shibuya');
  await expect(guide.locator('.pg-summary')).toHaveCount(0);
  await expect(guide.locator('.pg-activity-section')).toHaveCount(0);
  await expect(guide.locator('.pg-intro-section')).toHaveCount(0);

  await guide.getByText('Explore place guide', { exact: true }).click();
  await expect(guide.getByRole('heading', { name: 'Practical details' })).toBeVisible();
  await expect(guide).toContainText('2-24-12 Shibuya, Shibuya City');
});

test('the selected map stop carries its guide and trip note in both layouts', async ({ page, isMobile }) => {
  // The phone map defaults to the day's first stop. Omitting the stop deep link
  // there avoids also opening the timeline's selected-stop sheet underneath it.
  await page.goto(isMobile ? `${TRIP}/plan?view=map&day=d2` : `${TRIP}/plan?view=map&day=d2&stop=s-d2-meiji`);

  const selectedCard = isMobile
    ? page.getByRole('dialog', { name: 'Map' }).locator('.m-card')
    : page.locator('.map-popover');
  await expect(selectedCard).toBeVisible();
  const primaryBackground = await selectedCard
    .getByRole('button', { name: /Propose change/ })
    .evaluate((element) => getComputedStyle(element).backgroundColor);
  expect(primaryBackground).not.toBe('rgba(0, 0, 0, 0)');
  const guide = selectedCard.getByRole('region', { name: 'Guide to Meiji Jingū' });
  await expect(guide).toContainText(MEIJI_SUMMARY);
  await expect(guide.getByText('Trip note', { exact: true })).toBeVisible();
  await expect(guide).toContainText('Sunday morning = chance of seeing a traditional wedding procession.');

  if (isMobile) {
    await guide.getByText('Explore place guide', { exact: true }).click();
    await expect(guide.getByRole('heading', { name: 'Ideas while you’re here' })).toBeVisible();
    await expect(guide).toContainText(MEIJI_INTRO);
  } else {
    await expect(guide.getByRole('list', { name: 'Ideas while you’re here' })).toBeVisible();
    await expect(guide).toContainText('Walk beneath the large torii gates');

    await selectedCard.getByRole('button', { name: /Explore place guide/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Meiji Jingū' });
    const fullGuide = dialog.getByRole('region', { name: 'Guide to Meiji Jingū' });
    await expect(fullGuide).toContainText(MEIJI_INTRO);
    await expect(fullGuide.getByRole('heading', { name: 'Practical details' })).toBeVisible();
  }
});
