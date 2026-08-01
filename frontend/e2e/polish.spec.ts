import { test, expect } from '@playwright/test';

/** Coverage for the polish round — including the paths that would have caught
    the route bug: the proposal route toggle end-to-end, discussions, foldable
    candidates, candidate→plan, notice audiences, tinting, and the credit link. */

const TRIP = '/trips/t-japan26';

test('change composer defaults to poll and the button follows the route', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=change&stop=s-d1-hotel`);
  const route = page.getByRole('radiogroup', { name: 'Route' });
  const pollSeg = route.getByRole('radio', { name: 'Open a poll' });
  const leaderSeg = route.getByRole('radio', { name: 'Apply to the plan now' });
  await expect(pollSeg).toBeChecked();
  await expect(leaderSeg).not.toBeChecked();
  await expect(route.getByText('Publishes a new plan version immediately.')).toBeVisible();
  await expect(route.getByText('The group votes before the plan changes.')).toBeVisible();

  const pollChoice = pollSeg.locator('..');
  const leaderChoice = leaderSeg.locator('..');
  const appearance = await Promise.all(
    [pollChoice, leaderChoice].map((choice) =>
      choice.evaluate((el) => {
        const style = getComputedStyle(el);
        return {
          opacity: style.opacity,
          cursor: style.cursor,
          border: style.borderStyle,
          background: style.backgroundColor,
        };
      }),
    ),
  );
  expect(appearance[0].opacity).toBe('1');
  expect(appearance[1].opacity).toBe('1');
  expect(appearance[0].cursor).toBe('pointer');
  expect(appearance[1].cursor).toBe('pointer');
  expect(appearance[0].border).toBe('solid');
  expect(appearance[1].border).toBe('solid');
  expect(appearance[0].background).not.toBe(appearance[1].background);

  await expect(page.getByRole('button', { name: 'Open the poll →' })).toBeVisible();
  await leaderChoice.click();
  await expect(leaderSeg).toBeChecked();
  await expect(pollSeg).not.toBeChecked();
  await expect(page.getByRole('button', { name: 'Apply and publish →' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open the poll →' })).toHaveCount(0);

  // Native radios bring the expected arrow-key interaction too.
  await leaderSeg.focus();
  await page.keyboard.press('ArrowRight');
  await expect(pollSeg).toBeChecked();
});

test('a leader direct-routed proposal publishes a new plan version immediately', async ({ page }) => {
  await page.goto(`${TRIP}/plan?gov=change&stop=s-d1-hotel`);
  await page.locator('.compose select').first().selectOption('d2');

  const route = page.getByRole('radiogroup', { name: 'Route' });
  await route.getByRole('radio', { name: 'Apply to the plan now' }).locator('..').click();
  await expect(page.getByText('Preview · what will be published')).toBeVisible();
  await page.getByRole('button', { name: 'Apply and publish →' }).click();

  await expect(page.getByText('Plan updated ✓')).toBeVisible();
  await expect(page.getByText(/live in a newly published plan version/)).toBeVisible();
  await expect(page.getByText('See the applied change in Polls history.')).toBeVisible();
  await expect(page.getByText(/Sent to leaders|leader will approve/i)).toHaveCount(0);
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
  await dock.getByRole('radio', { name: 'Apply to the plan now' }).locator('..').click();
  await expect(dock.getByRole('button', { name: 'Apply and publish →' })).toBeVisible();
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
  const passedOn = page.getByRole('button', { name: /Passed on/ });
  await expect(passedOn).toHaveAttribute('aria-expanded', 'false');
  const passedOnBody = page.locator('.cand-section', { has: passedOn }).locator('.cand-section-body');
  expect((await passedOnBody.boundingBox())?.height ?? 99).toBeLessThan(8);
  await passedOn.click();
  await expect(passedOn).toHaveAttribute('aria-expanded', 'true');
  await expect.poll(async () => (await passedOnBody.boundingBox())?.height ?? 0).toBeGreaterThan(50);
  const considering = page.getByRole('button', { name: /Ideas to consider/ });
  await expect(considering).toHaveAttribute('aria-expanded', 'true');
  const consideringBody = page.locator('.cand-section', { has: considering }).locator('.cand-section-body');
  await considering.click();
  await expect(considering).toHaveAttribute('aria-expanded', 'false');
  await expect.poll(async () => (await consideringBody.boundingBox())?.height ?? 0).toBeLessThan(8);
});

test('propose a shortlisted candidate for the plan', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  await page
    .getByRole('button', { name: /Propose for a day/ })
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

test('passing on an idea uses a clear, reversible confirmation', async ({ page, isMobile }) => {
  await page.goto(`${TRIP}/candidates`);
  const ghibli = page.locator('.cand-card').filter({ hasText: 'Ghibli Museum' });

  await ghibli.getByRole('button', { name: 'Pass on this idea' }).click();
  let dialog = page.getByRole('dialog', { name: 'Pass on Ghibli Museum?' });
  await expect(dialog).toContainText('This moves the idea out of active consideration and into Passed on');
  await expect(dialog).toContainText('This can be undone. Anyone can reconsider it later');
  await dialog.evaluate(async (el) => {
    await Promise.all(el.getAnimations().map((animation) => animation.finished));
  });

  const close = dialog.getByRole('button', { name: 'Close' });
  const cancel = dialog.getByRole('button', { name: 'Keep considering' });
  const confirm = dialog.getByRole('button', { name: 'Pass on idea' });
  const closeBox = (await close.boundingBox())!;
  expect(closeBox.width).toBeGreaterThanOrEqual(44);
  expect(closeBox.height).toBeGreaterThanOrEqual(44);

  const controls = await dialog.evaluate((el) => {
    const cancelEl = el.querySelector<HTMLElement>('.cand-reject-cancel')!;
    const confirmEl = el.querySelector<HTMLElement>('.cand-reject-confirm')!;
    const actions = el.querySelector<HTMLElement>('.cand-reject-actions')!;
    return {
      cancelBackground: getComputedStyle(cancelEl).backgroundColor,
      confirmBackground: getComputedStyle(confirmEl).backgroundColor,
      actionsBackground: getComputedStyle(actions).backgroundColor,
      actionsPaddingBottom: Number.parseFloat(getComputedStyle(actions).paddingBottom),
    };
  });
  expect(controls.cancelBackground).not.toBe(controls.confirmBackground);
  expect(controls.actionsBackground).not.toBe('rgba(0, 0, 0, 0)');
  expect(controls.actionsPaddingBottom).toBeGreaterThanOrEqual(12);

  if (isMobile) {
    const cancelBox = (await cancel.boundingBox())!;
    const confirmBox = (await confirm.boundingBox())!;
    expect(confirmBox.y).toBeGreaterThan(cancelBox.y);
    expect(confirmBox.y + confirmBox.height).toBeLessThanOrEqual(page.viewportSize()!.height);
    await expect(dialog.locator('.cand-reject-grip')).toBeVisible();
  }

  await dialog.getByRole('button', { name: 'Keep considering' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(ghibli.getByRole('button', { name: 'Pass on this idea' })).toBeVisible();

  await ghibli.getByRole('button', { name: 'Pass on this idea' }).click();
  dialog = page.getByRole('dialog', { name: 'Pass on Ghibli Museum?' });
  await dialog.getByRole('button', { name: 'Pass on idea' }).click();
  await expect(dialog).not.toBeVisible();

  const passedOn = page.getByRole('button', { name: /^Passed on/ });
  await expect(passedOn).toHaveAttribute('aria-expanded', 'true');
  const passedOnSection = page.locator('.cand-section', { has: passedOn });
  await expect(passedOnSection.locator('.cand-card').filter({ hasText: 'Ghibli Museum' })).toBeVisible();
});

test('route and decision hierarchy remain clear in Simplified Chinese dark mode', async ({ page }) => {
  await page.goto(`${TRIP}/candidates`);
  await page.getByRole('radio', { name: 'Dark' }).click();
  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  const ghibli = page.locator('.cand-card').filter({ hasText: 'Ghibli Museum' });
  await ghibli.locator('.cand-actions button').last().click();
  const dialog = page.locator('.cand-reject-modal');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('这会将该灵感移出当前候选，并放入“已放弃”');
  await expect(dialog).toContainText('此操作可以撤销，之后任何人都可以重新考虑');

  const decisionColours = await dialog.evaluate((el) => ({
    secondary: getComputedStyle(el.querySelector<HTMLElement>('.cand-reject-cancel')!).backgroundColor,
    destructive: getComputedStyle(el.querySelector<HTMLElement>('.cand-reject-confirm')!).backgroundColor,
  }));
  expect(decisionColours.secondary).not.toBe(decisionColours.destructive);
  await dialog.locator('.cand-reject-close').click();
  await expect(dialog).not.toBeVisible();

  await page.goto(`${TRIP}/plan?gov=change&stop=s-d1-hotel`);
  const route = page.getByRole('radiogroup', { name: '审批方式' });
  const leader = route.getByRole('radio', { name: '立即应用到计划' });
  const poll = route.getByRole('radio', { name: '发起投票' });
  await expect(route.getByText('立即发布一个新的计划版本。')).toBeVisible();
  await expect(poll).toBeChecked();
  await leader.locator('..').click();
  await expect(leader).toBeChecked();
  await expect(poll).not.toBeChecked();
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
  const section = page.getByRole('heading', { name: 'Ideas to consider' });
  expect(await section.evaluate((el) => getComputedStyle(el).fontSize)).toBe('20px');
  expect(
    await page
      .getByRole('button', { name: /Propose for a day/ })
      .first()
      .evaluate((el) => getComputedStyle(el).fontSize),
  ).toBe('14px');

  await page
    .getByRole('button', { name: /Propose for a day/ })
    .first()
    .click();
  const dialog = page.getByRole('dialog', { name: /Propose a stop/ });
  // Route radios are visually represented by their full-size choice cards;
  // only text/value controls render their own font inside the native element.
  const fields = dialog.locator('input:not([type="radio"]), select, textarea');
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
