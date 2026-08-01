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

test('the mobile proposal sheet ends in a distinct pinned action dock', async ({ page, isMobile }) => {
  test.skip(!isMobile, 'mobile proposal sheet');
  await page.goto(`${TRIP}/plan?gov=addStop&day=d2&mode=candidates&candidate=c-ghibli`);

  const dialog = page.getByRole('dialog', { name: /Propose a stop/ });
  const body = dialog.locator('.compose-body');
  const dock = dialog.locator('.compose-dock');
  await expect(dock.getByText('Route', { exact: true })).toBeVisible();
  await expect(dock.getByRole('button', { name: 'Cancel' })).toBeVisible();
  await expect(dock.getByRole('button', { name: /Open the poll/ }).last()).toBeVisible();
  await dialog.evaluate(async (el) => {
    await Promise.all(el.getAnimations().map((animation) => animation.finished));
  });

  const material = await dock.evaluate((el) => {
    const style = getComputedStyle(el);
    return { background: style.backgroundColor, border: style.borderTopStyle, shadow: style.boxShadow };
  });
  expect(material.background).not.toBe('rgba(0, 0, 0, 0)');
  expect(material.border).toBe('solid');
  expect(material.shadow).not.toBe('none');

  const before = (await dock.boundingBox())!;
  await body.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await expect.poll(() => body.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  const after = (await dock.boundingBox())!;
  expect(after.y).toBeCloseTo(before.y, 0);
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
  // An empty state's job is to hand you the first sentence, so it names the
  // stop and offers three things worth saying about it.
  await expect(page.locator('.thread-empty')).toContainText('Nobody has said anything about');
  await expect(page.locator('.thread-empty')).toContainText('Omoide Yokocho');
  await expect(page.locator('.thread-empty li')).toHaveCount(3);
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
  expect((await votedBody.boundingBox())?.height ?? 99).toBeLessThan(8);
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
  await page
    .getByRole('button', { name: /Propose for the plan/ })
    .first()
    .click();
  await expect(page).toHaveURL(`${TRIP}/candidates`);

  const dialog = page.getByRole('dialog', { name: /Propose a stop/ });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Ghibli Museum' })).toHaveAttribute('aria-pressed', 'true');
  const daySelect = dialog.locator('.field select').first();
  await expect(daySelect).toBeVisible();
  await expect(daySelect.locator('option').first()).toContainText('Day 1');
  await dialog
    .getByRole('button', { name: /Open the poll/ })
    .last()
    .click();
  await expect(page.getByRole('dialog', { name: /Poll opened/ })).toBeVisible();
  await expect(page).toHaveURL(`${TRIP}/candidates`);
});

test('moving an idea out of the running requires confirmation', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  const ghibli = page.locator('.cand-card').filter({ hasText: 'Ghibli Museum' });

  await ghibli.getByRole('button', { name: 'Not for this trip' }).click();
  let dialog = page.getByRole('dialog', { name: /Move Ghibli Museum out of the running/ });
  await expect(dialog).toContainText('You can bring it back later');
  await dialog.getByRole('button', { name: 'Keep candidate' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(ghibli.getByRole('button', { name: 'Not for this trip' })).toBeVisible();

  await ghibli.getByRole('button', { name: 'Not for this trip' }).click();
  dialog = page.getByRole('dialog', { name: /Move Ghibli Museum out of the running/ });
  await dialog.getByRole('button', { name: 'Move to Voted off' }).click();
  await expect(dialog).not.toBeVisible();

  const votedOff = page.getByRole('button', { name: /^Voted off/ });
  await expect(votedOff).toHaveAttribute('aria-expanded', 'true');
  const votedSection = page.locator('.cand-section', { has: votedOff });
  await expect(votedSection.locator('.cand-card').filter({ hasText: 'Ghibli Museum' })).toBeVisible();
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

test('header credits the author without crowding the mobile app bar', async ({ page, isMobile }) => {
  await page.goto('/');
  const linkedCredit = page.locator('a[href="https://github.com/Kaiyu2023/itinera"]');
  const plainCredit = page.locator('.mobile-credit');

  if (isMobile) {
    await expect(linkedCredit).not.toBeVisible();
    await expect(plainCredit).toBeVisible();
    await expect(plainCredit).toHaveText('By Kaiyu2023');
  } else {
    await expect(linkedCredit).toBeVisible();
    await expect(linkedCredit).toHaveText('By Kaiyu2023');
    await expect(plainCredit).not.toBeVisible();
  }

  const topbar = page.locator('.topbar');
  const sizing = await topbar.evaluate((el) => ({
    clientWidth: el.clientWidth,
    scrollWidth: el.scrollWidth,
    height: el.getBoundingClientRect().height,
  }));
  expect(sizing.scrollWidth).toBeLessThanOrEqual(sizing.clientWidth);
  expect(sizing.height).toBe(56);
});

test('page titles, controls, and form values use the shared type roles', async ({ page, isMobile }) => {
  const titleSizes: string[] = [];
  for (const tab of ['candidates', 'polls', 'ledger', 'prep']) {
    await page.goto(`${TRIP}/${tab}`);
    const title = page.locator('.m4-tab-head').getByRole('heading', { level: 2 });
    await expect(title).toBeVisible();
    titleSizes.push(await title.evaluate((el) => getComputedStyle(el).fontSize));
  }
  expect(new Set(titleSizes).size).toBe(1);
  expect(titleSizes[0]).toBe(isMobile ? '24px' : '26px');

  await page.goto(`${TRIP}/candidates`);
  const section = page.getByRole('heading', { name: 'Competing for a slot' });
  expect(await section.evaluate((el) => getComputedStyle(el).fontSize)).toBe('20px');
  expect(
    await page
      .getByRole('button', { name: /Propose for the plan/ })
      .first()
      .evaluate((el) => getComputedStyle(el).fontSize),
  ).toBe('14px');

  await page
    .getByRole('button', { name: /Propose for the plan/ })
    .first()
    .click();
  const dialog = page.getByRole('dialog', { name: /Propose a stop/ });
  const fields = dialog.locator('input, select, textarea');
  for (let i = 0; i < (await fields.count()); i += 1) {
    expect(await fields.nth(i).evaluate((el) => getComputedStyle(el).fontSize)).toBe('16px');
  }
});

test('trip pages tint the background with the trip accent', async ({ page }) => {
  await page.goto(`${TRIP}/plan`);
  await expect(page.locator('body')).toHaveClass(/trip-tinted/);
  await page.goto('/');
  await expect(page.locator('body')).not.toHaveClass(/trip-tinted/);
});
