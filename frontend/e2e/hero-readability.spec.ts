import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

const TRIP = '/trips/t-japan26/plan?view=timeline';
const AUTHORED_TITLE = 'Japan, Autumn Leaves';

async function expectInside(outer: Locator, inner: Locator) {
  const [outerBox, innerBox] = await Promise.all([outer.boundingBox(), inner.boundingBox()]);
  expect(outerBox).not.toBeNull();
  expect(innerBox).not.toBeNull();
  if (!outerBox || !innerBox) return;

  // Allow a pixel for fractional layout/device-scale rounding.
  expect(innerBox.x).toBeGreaterThanOrEqual(outerBox.x - 1);
  expect(innerBox.y).toBeGreaterThanOrEqual(outerBox.y - 1);
  expect(innerBox.x + innerBox.width).toBeLessThanOrEqual(outerBox.x + outerBox.width + 1);
  expect(innerBox.y + innerBox.height).toBeLessThanOrEqual(outerBox.y + outerBox.height + 1);
}

async function expectHeroHasNoHorizontalOverflow(page: Page) {
  const geometry = await page.locator('.trip-hero').evaluate((hero) => {
    const body = hero.querySelector<HTMLElement>('.body');
    return {
      heroScrollWidth: hero.scrollWidth,
      heroClientWidth: hero.clientWidth,
      bodyScrollWidth: body?.scrollWidth ?? 0,
      bodyClientWidth: body?.clientWidth ?? 0,
      documentScrollWidth: document.documentElement.scrollWidth,
      viewportWidth: window.innerWidth,
    };
  });

  expect(geometry.heroScrollWidth).toBeLessThanOrEqual(geometry.heroClientWidth + 1);
  expect(geometry.bodyScrollWidth).toBeLessThanOrEqual(geometry.bodyClientWidth + 1);
  expect(geometry.documentScrollWidth).toBeLessThanOrEqual(geometry.viewportWidth + 1);
}

test('the trip hero owns a readable copy field in both UI languages', async ({ page }) => {
  await page.goto(TRIP);

  const hero = page.locator('.trip-hero');
  const body = hero.locator('.body');
  const title = hero.getByRole('heading', { name: AUTHORED_TITLE, exact: true });
  const meta = hero.locator('.on-photo-meta');

  await expect(hero).toBeVisible();
  await expect(title).toHaveText(AUTHORED_TITLE);
  await expect(meta).toBeVisible();

  const contract = await body.evaluate((element) => {
    const bodyStyle = getComputedStyle(element);
    const scrim = getComputedStyle(element, '::before');
    const colors = scrim.backgroundImage.match(/rgba?\([^)]*\)/g) ?? [];
    const alphas = colors.flatMap((color) => {
      const slashAlpha = color.match(/\/\s*([\d.]+)\s*\)$/);
      const commaAlpha = color.match(/,\s*([\d.]+)\s*\)$/);
      const value = slashAlpha?.[1] ?? commaAlpha?.[1];
      return value == null ? [] : [Number(value)];
    });

    return {
      bodyPosition: bodyStyle.position,
      bodyIsolation: bodyStyle.isolation,
      bodyZIndex: bodyStyle.zIndex,
      content: scrim.content,
      position: scrim.position,
      pointerEvents: scrim.pointerEvents,
      zIndex: scrim.zIndex,
      top: Number.parseFloat(scrim.top),
      right: Number.parseFloat(scrim.right),
      bottom: Number.parseFloat(scrim.bottom),
      left: Number.parseFloat(scrim.left),
      backgroundImage: scrim.backgroundImage,
      alphas,
    };
  });

  // The reading field belongs to `.body`, so it grows upward when localized
  // metadata or a long authored title makes the content taller.
  expect(contract.bodyPosition).toBe('relative');
  expect(contract.bodyIsolation).toBe('isolate');
  expect(contract.bodyZIndex).toBe('0');
  expect(contract.content).not.toBe('none');
  expect(contract.position).toBe('absolute');
  expect(contract.pointerEvents).toBe('none');
  expect(contract.zIndex).toBe('-1');
  expect(contract.top).toBeLessThan(0);
  expect(contract.right).toBeCloseTo(0, 0);
  expect(contract.bottom).toBeCloseTo(0, 0);
  expect(contract.left).toBeCloseTo(0, 0);
  expect(contract.backgroundImage).toContain('radial-gradient');
  // The inner reading area is dark enough for small white metadata, while the
  // outer edge fades almost entirely away so the cover is not globally dimmed.
  expect(contract.alphas.some((alpha) => alpha >= 0.64)).toBe(true);
  expect(contract.alphas.some((alpha) => alpha <= 0.1)).toBe(true);

  const copyStyles = await hero.evaluate((element) => {
    const heading = element.querySelector('h1');
    const metadata = element.querySelector('.on-photo-meta');
    if (!heading || !metadata) throw new Error('Hero copy is missing');
    const titleStyle = getComputedStyle(heading);
    const metaStyle = getComputedStyle(metadata);
    return {
      overflow: getComputedStyle(element).overflow,
      titleColor: titleStyle.color,
      titleShadow: titleStyle.textShadow,
      metaColor: metaStyle.color,
      metaShadow: metaStyle.textShadow,
    };
  });

  expect(copyStyles.overflow).toBe('hidden');
  expect(copyStyles.titleColor).toBe('rgb(255, 255, 255)');
  expect(copyStyles.metaColor).toBe('rgb(255, 255, 255)');
  expect(copyStyles.titleShadow).not.toBe('none');
  expect(copyStyles.metaShadow).not.toBe('none');

  await expectInside(hero, body);
  await expectInside(hero, title);
  await expectInside(hero, meta);
  await expectHeroHasNoHorizontalOverflow(page);

  await page.getByRole('button', { name: 'Switch UI language to Simplified Chinese' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');

  // Locale changes product chrome and date formatting, never API-authored text.
  await expect(title).toHaveText(AUTHORED_TITLE);
  await expect(meta).toContainText('11月14日周六');
  await expectInside(hero, title);
  await expectInside(hero, meta);
  await expectHeroHasNoHorizontalOverflow(page);
});
