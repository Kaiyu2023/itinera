import { test, expect } from '@playwright/test';

const POLLS = '/trips/t-japan26/polls';

test('closing a plan-change poll previews tally, quorum and exact plan-version impact', async ({ page, isMobile }) => {
  await page.goto(POLLS);
  const poll = page.locator('.poll', { hasText: "Restructure Day 6 (Makoto's proposal)" });
  await poll.getByRole('button', { name: 'Close now' }).click();

  const dialog = page.getByRole('dialog', { name: "Close “Restructure Day 6 (Makoto's proposal)” now?" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('3 of 6 voted')).toBeVisible();
  await expect(dialog.getByText('3 required · met')).toBeVisible();
  await expect(dialog.getByText('“Adopt the change” leads with 2')).toBeVisible();
  await expect(dialog.getByText('Publishes Plan v4 immediately and replaces current Plan v3.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Close and publish Plan v4' })).toBeEnabled();
  await expect(dialog.locator(':focus')).toHaveCount(1);

  if (isMobile) {
    await dialog.evaluate(async (element) => {
      await Promise.all(element.getAnimations().map((animation) => animation.finished));
    });
    const box = (await dialog.boundingBox())!;
    const viewport = page.viewportSize()!;
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(viewport.height);
    for (const action of await dialog.locator('.poll-close-actions .btn').all()) {
      expect((await action.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    }
  }

  await dialog.getByRole('button', { name: 'Keep voting' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(poll.getByText('open', { exact: true })).toBeVisible();
  await expect(poll.getByRole('button', { name: 'Close now' })).toBeVisible();
});

test('confirming a plan-change close publishes the version named by the action', async ({ page }) => {
  await page.goto(POLLS);
  const poll = page.locator('.poll', { hasText: "Restructure Day 6 (Makoto's proposal)" });
  await poll.getByRole('button', { name: 'Close now' }).click();
  const dialog = page.getByRole('dialog', { name: "Close “Restructure Day 6 (Makoto's proposal)” now?" });
  await dialog.getByRole('button', { name: 'Close and publish Plan v4' }).click();

  await expect(dialog).not.toBeVisible();
  await expect(poll.getByText('passed', { exact: true })).toBeVisible();
  await expect(poll.getByText('winner', { exact: true })).toBeVisible();
  await expect(poll.getByRole('button', { name: 'Close now' })).toHaveCount(0);
});

test('an author or a trip leader gets an active open action with an explanation', async ({ page }) => {
  await page.goto(POLLS);

  const authoredDraft = page.locator('.poll', { hasText: 'Nara or Uji for the Day 5 day-trip?' });
  await expect(authoredDraft.getByText('You created this poll, so you can open it when it’s ready.')).toBeVisible();
  await expect(authoredDraft.getByRole('button', { name: 'Open poll' })).toBeEnabled();

  const scheduledByAnotherMember = page.locator('.poll', { hasText: 'Osaka finale: USJ or a Dōtonbori day?' });
  await expect(scheduledByAnotherMember.getByText('Trip leaders can open this poll when it’s ready.')).toBeVisible();
  await expect(scheduledByAnotherMember.getByRole('button', { name: 'Open now' })).toBeEnabled();
});

test('a tied close is presented as no decision, never as a winner', async ({ page }) => {
  await page.goto(POLLS);
  const poll = page.locator('.poll', { hasText: 'Day 2 dinner in Shibuya' });
  await poll.getByRole('radio', { name: 'Uobei (bullet-train sushi, cheap) — 1 vote' }).click();
  await poll.getByRole('button', { name: 'Close now' }).click();

  const dialog = page.getByRole('dialog', { name: 'Close “Day 2 dinner in Shibuya” now?' });
  await expect(dialog.getByText('Tie at 2 votes')).toBeVisible();
  await expect(dialog.getByText('The top options are tied, so closing records no decision.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Close with no decision' }).click();

  await expect(dialog).not.toBeVisible();
  await expect(poll.getByText('no decision', { exact: true })).toBeVisible();
  await expect(poll.getByText('No decision', { exact: true })).toBeVisible();
  await expect(poll.locator('.opt.win')).toHaveCount(0);
  await expect(poll.getByText('winner', { exact: true })).toHaveCount(0);
});

test('the close preview and actions are localized while authored poll copy stays unchanged', async ({ page }) => {
  await page.goto(POLLS);
  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();
  const poll = page.locator('.poll', { hasText: "Restructure Day 6 (Makoto's proposal)" });
  await poll.getByRole('button', { name: '立即关闭' }).click();

  const dialog = page.getByRole('dialog', { name: "立即关闭“Restructure Day 6 (Makoto's proposal)”？" });
  await expect(dialog.getByText('3/6 人已投票')).toBeVisible();
  await expect(dialog.getByText('立即发布计划 v4，并替换当前的计划 v3。')).toBeVisible();
  await expect(dialog.getByRole('button', { name: '关闭并发布计划 v4' })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: '继续投票' })).toBeEnabled();
});
