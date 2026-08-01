import { expect, test } from '@playwright/test';

test('the language switch translates UI chrome, preserves authored content, and persists', async ({ page }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');

  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();

  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.getByRole('link', { name: '计划', exact: true }).filter({ visible: true }).first()).toBeVisible();
  // Trip names come from the API, so changing UI language must not rewrite them.
  await expect(page.getByRole('heading', { name: 'Japan, Autumn Leaves' })).toBeVisible();

  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.getByRole('button', { name: '将界面语言切换为 English' })).toBeVisible();
});

test('Simplified Chinese covers every routed product area', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();

  const routes = [
    ['/', '我的行程'],
    ['/review', '你的审核队列'],
    ['/trips/t-japan26/candidates', '行程灵感'],
    ['/trips/t-japan26/polls', '共同决策'],
    ['/trips/t-japan26/ledger', '账本'],
    ['/trips/t-japan26/prep', '行前准备'],
  ] as const;

  for (const [path, heading] of routes) {
    await page.goto(path);
    await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
    await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
  }

  // API/authored content stays byte-for-byte unchanged on localized pages.
  await page.goto('/trips/t-japan26/candidates');
  await expect(page.getByText('Ghibli Museum', { exact: true }).first()).toBeVisible();
});

test('an already-mounted map updates provider chrome when the language changes', async ({ page, isMobile }) => {
  await page.goto('/trips/t-japan26/plan?view=map&day=d2');
  await expect(page.getByRole('button', { name: 'Zoom in' }).first()).toBeVisible();

  const languageToggle = page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' });
  if (isMobile) {
    // The full-screen map correctly owns pointer input while open. Dispatch the
    // same button action directly so this case can exercise its live locale
    // subscription without dismissing/remounting the map.
    await languageToggle.evaluate((button: HTMLButtonElement) => button.click());
  } else {
    await languageToggle.click();
  }

  await expect(page.getByRole('button', { name: '放大地图' }).first()).toBeVisible();
  await expect(page.getByText('风格化占位地图 — MockMapRenderer').first()).toBeVisible();
  await expect(page.getByText('Meiji Jingū', { exact: true }).first()).toBeVisible();
});

test.describe('first-run locale negotiation', () => {
  test.use({ locale: 'zh-CN' });

  test('a Simplified-Chinese browser starts in Simplified Chinese', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  });
});

test.describe('Traditional-Chinese locale fallback', () => {
  test.use({ locale: 'zh-TW' });

  test('a Traditional-Chinese browser keeps the English fallback', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  });
});
