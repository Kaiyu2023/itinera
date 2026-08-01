import { expect, test } from '@playwright/test';

/** A brand-new trip must be able to move from an empty Plan to its first
    governed stop without a refresh, hidden prerequisite, or dead CTA. */
test('the first trip idea creates dated days and opens a real proposal', async ({ page, isMobile }) => {
  await page.goto('/?trip=new');
  const createTrip = page.getByRole('dialog', { name: 'New trip' });
  await createTrip.getByLabel('Name').fill('Tokyo, First Draft');
  await createTrip.getByLabel('Start date').fill('2027-04-02');
  await createTrip.getByLabel('End date').fill('2027-04-04');
  await createTrip.getByRole('button', { name: 'Create trip' }).click();

  await expect(page).toHaveURL(/\/trips\/t-[^/]+\/plan$/);
  const onboarding = page.locator('.plan-zero');
  await expect(onboarding.getByRole('heading', { name: 'Turn one idea into a day-by-day plan' })).toBeVisible();
  await expect(onboarding.getByRole('listitem')).toHaveCount(3);
  await expect(onboarding).toContainText('Save a place in Trip ideas');
  await expect(onboarding).toContainText('Pick the date and approval route');

  const box = await onboarding.evaluate((element) => ({
    right: element.getBoundingClientRect().right,
    viewport: document.documentElement.clientWidth,
  }));
  expect(box.right).toBeLessThanOrEqual(box.viewport + 1);
  if (isMobile) {
    const first = (await onboarding.getByRole('listitem').first().boundingBox())!;
    const second = (await onboarding.getByRole('listitem').nth(1).boundingBox())!;
    expect(second.y).toBeGreaterThan(first.y + first.height - 1);
  }

  await onboarding.getByRole('link', { name: 'Add your first idea' }).click();
  const ideaComposer = page.getByRole('dialog', { name: 'Add a trip idea' });
  await expect(ideaComposer).toBeVisible();
  await ideaComposer.getByRole('button', { name: 'Enter manually' }).click();
  await ideaComposer.getByLabel('Name').fill('Kiyosumi Gardens');
  await ideaComposer.getByLabel('City').fill('Tokyo');
  await ideaComposer.getByLabel('Why suggest this place').fill('A calm first afternoon after the flight.');
  await ideaComposer.getByRole('button', { name: 'Add to trip ideas' }).click();

  const idea = page.locator('.cand-card').filter({ hasText: 'Kiyosumi Gardens' });
  await expect(idea).toBeVisible();
  await idea.getByRole('button', { name: 'Propose for a day' }).click();

  const proposal = page.getByRole('dialog', { name: /Propose a stop/ });
  await expect(proposal).toBeVisible();
  await expect(proposal.getByRole('button', { name: 'Kiyosumi Gardens' })).toHaveAttribute('aria-pressed', 'true');
  const day = proposal.locator('.field select').first();
  await expect(day.locator('option')).toHaveCount(3);
  await expect(day.locator('option').first()).toContainText('Day 1');
  await expect(day.locator('option').first()).toContainText('Tokyo');
  await day.selectOption({ index: 1 });
  await proposal.locator('.route-option', { hasText: 'Apply to the plan now' }).click();
  await expect(proposal.getByRole('radio', { name: 'Apply to the plan now' })).toBeChecked();
  await proposal.getByRole('button', { name: 'Apply and publish →' }).click();

  const sent = page.getByRole('dialog', { name: /Plan updated/ });
  await expect(sent).toBeVisible();
  await sent.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByRole('button', { name: 'Propose for a day' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Added to the plan/ })).toBeVisible();

  // Use the app's navigation: a hard reload intentionally resets the in-memory
  // Phase-A backend, while the real user flow remains inside the SPA.
  await page.locator('a[href$="/plan"]:visible').first().click();
  await expect(page.getByText('Plan v2 · 3 days · 1 stops')).toBeVisible();
  if (!isMobile) {
    await page.getByRole('tab', { name: 'Timeline' }).click();
  }
  await expect(page.locator('.day-scrubber .day-chip')).toHaveCount(3);
});
