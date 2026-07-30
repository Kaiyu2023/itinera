import { test, expect } from '@playwright/test';
import type { Page, Route } from '@playwright/test';

/**
 * Weather is fetched, not stored — same standing as sunrise/sunset. These tests
 * stub Open-Meteo rather than calling it, for the obvious reason (a suite that
 * needs the internet is a suite that fails on a train) and one less obvious
 * one: the interesting assertion is about *which claim the UI makes*, and a
 * live API would hand us a different answer every run.
 *
 * Every trip in the fixtures is months out, so the forecast horizon is never
 * reached and the climatology path is the one that matters.
 */

const RAIN = 61; // WMO: rain, slight

/** Stand in for the reanalysis archive, echoing back the range that was asked
    for so the caller's index alignment is exercised for real. */
async function stubArchive(page: Page, opts: { code?: number; max?: number; min?: number; mm?: number } = {}) {
  const { code = RAIN, max = 14.2, min = 6.4, mm = 3.1 } = opts;
  await page.route('**://archive-api.open-meteo.com/**', async (route: Route) => {
    const url = new URL(route.request().url());
    const count = url.searchParams.get('latitude')!.split(',').length;
    const start = new Date(`${url.searchParams.get('start_date')}T00:00:00Z`);
    const end = new Date(`${url.searchParams.get('end_date')}T00:00:00Z`);
    const days = Math.round((end.valueOf() - start.valueOf()) / 86_400_000) + 1;
    const time = Array.from({ length: days }, (_, i) =>
      new Date(start.valueOf() + i * 86_400_000).toISOString().slice(0, 10),
    );
    const block = {
      daily: {
        time,
        weather_code: time.map(() => code),
        temperature_2m_max: time.map(() => max),
        temperature_2m_min: time.map(() => min),
        precipitation_sum: time.map(() => mm),
      },
    };
    await route.fulfill({ json: Array.from({ length: count }, () => block) });
  });
}

/** The forecast endpoint must never be reached for a trip this far out. */
async function failForecast(page: Page) {
  await page.route('**://api.open-meteo.com/**', (route) => route.abort());
}

test('a date beyond the forecast horizon is labelled as typical, not forecast', async ({ page }) => {
  await stubArchive(page);
  await failForecast(page);
  await page.goto('/trips/t-japan26/plan?view=timeline&day=d6');

  const wx = page.locator('.day-wx');
  await expect(wx).toBeVisible();
  // The distinction is the feature: a multi-year median printed as a forecast
  // is a lie told to someone packing a bag.
  await expect(wx).toHaveClass(/typical/);
  await expect(wx).toContainText('typical');
  await expect(wx).not.toContainText('forecast');
  await expect(wx).toContainText('14° / 6°');
  await expect(wx).toContainText('rain');
  await expect(wx.getByRole('img', { name: 'rain' })).toBeVisible();
});

test('every ribbon day carries the same reading as its day view', async ({ page }) => {
  await stubArchive(page, { code: 0, max: 19.6, min: 9.1, mm: 0 });
  await failForecast(page);
  await page.goto('/trips/t-japan26/plan?view=timeline');

  const chips = page.locator('.ribbon .rb-wx');
  await expect(chips).toHaveCount(7);
  await expect(chips.first()).toContainText('20°');
  await expect(chips.first()).toHaveClass(/typical/);
  await expect(page.locator('.day-wx')).toContainText('20° / 9°');
  await expect(page.locator('.day-wx')).toContainText('clear');
});

test('the plan renders unchanged when the weather service is unreachable', async ({ page }) => {
  // Weather is decoration on a plan that has to work in a tunnel. It may never
  // block a render, retry hard, or throw.
  await page.route('**open-meteo.com/**', (route) => route.abort());
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));

  await page.goto('/trips/t-japan26/plan?view=timeline&day=d6');
  await expect(page.locator('.daycanvas')).toBeVisible();
  await expect(page.locator('.dc-blk').filter({ hasText: 'Fushimi Inari' })).toBeVisible();
  await expect(page.locator('.day-wx')).toHaveCount(0);
  await expect(page.locator('.rb-wx')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('a second visit is served from storage without touching the network', async ({ page }) => {
  let calls = 0;
  await page.route('**://archive-api.open-meteo.com/**', async (route) => {
    calls++;
    await stubOnce(route);
  });
  await failForecast(page);

  await page.goto('/trips/t-japan26/plan?view=timeline&day=d6');
  await expect(page.locator('.day-wx')).toBeVisible();
  const first = calls;
  expect(first).toBeGreaterThan(0);

  await page.goto('/trips/t-japan26/plan?view=timeline&day=d1');
  await expect(page.locator('.day-wx')).toBeVisible();
  // A five-year median for a week in November does not move, and this app is
  // meant to open on roaming data.
  expect(calls).toBe(first);
});

async function stubOnce(route: Route) {
  const url = new URL(route.request().url());
  const count = url.searchParams.get('latitude')!.split(',').length;
  const start = new Date(`${url.searchParams.get('start_date')}T00:00:00Z`);
  const end = new Date(`${url.searchParams.get('end_date')}T00:00:00Z`);
  const days = Math.round((end.valueOf() - start.valueOf()) / 86_400_000) + 1;
  const time = Array.from({ length: days }, (_, i) =>
    new Date(start.valueOf() + i * 86_400_000).toISOString().slice(0, 10),
  );
  await route.fulfill({
    json: Array.from({ length: count }, () => ({
      daily: {
        time,
        weather_code: time.map(() => 3),
        temperature_2m_max: time.map(() => 11),
        temperature_2m_min: time.map(() => 4),
        precipitation_sum: time.map(() => 0),
      },
    })),
  });
}
