/**
 * Screenshot sweep for design review — not a test.
 *
 *   node e2e/shots.mjs [baseURL] [outDir]
 *
 * Walks every surface at both breakpoints in both colour schemes, because every
 * interesting failure in this app is a failure in one of those four quadrants
 * and invisible in the other three. The list below is the flow inventory, not a
 * sample: a route that is not here is a route nobody is looking at.
 *
 * Weather is stubbed. Not for determinism — the plan renders fine without it —
 * but because a sweep should not put a hundred requests into a free public API
 * to take the same picture four times.
 */
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';

const BASE = process.argv[2] ?? 'http://127.0.0.1:4180';
const OUT = process.argv[3] ?? '/tmp/shots';

const VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 900 },
  { name: 'mobile', width: 390, height: 844 },
];

/** `full` = capture the whole scroll height, for views taller than the screen. */
const ROUTES = [
  { name: 'trips', path: '/' },
  { name: 'trips-new', path: '/?trip=new' },
  { name: 'review', path: '/review' },

  { name: 'plan-day1', path: '/trips/t-japan26/plan?view=timeline&day=d1', full: true },
  { name: 'plan-day3', path: '/trips/t-japan26/plan?view=timeline&day=d3', full: true },
  { name: 'plan-day6', path: '/trips/t-japan26/plan?view=timeline&day=d6', full: true },
  { name: 'plan-day7', path: '/trips/t-japan26/plan?view=timeline&day=d7', full: true },
  { name: 'plan-map', path: '/trips/t-japan26/plan?view=map' },
  { name: 'plan-addstop', path: '/trips/t-japan26/plan?view=timeline&gov=addStop&day=d1' },
  { name: 'plan-change', path: '/trips/t-japan26/plan?view=timeline&gov=change&stop=s-d1-omoide' },
  { name: 'plan-discuss', path: '/trips/t-japan26/plan?view=timeline&gov=discuss&stop=s-d1-omoide' },
  { name: 'plan-editstop', path: '/trips/t-japan26/plan?view=timeline&edit=stop:s-d1-omoide' },
  { name: 'plan-editday', path: '/trips/t-japan26/plan?view=timeline&edit=day:d1' },

  { name: 'candidates', path: '/trips/t-japan26/candidates', full: true },
  { name: 'candidates-new', path: '/trips/t-japan26/candidates?cand=new&q=shibuya&pick=first' },
  { name: 'polls', path: '/trips/t-japan26/polls', full: true },
  { name: 'polls-new', path: '/trips/t-japan26/polls?poll=new' },
  { name: 'ledger', path: '/trips/t-japan26/ledger', full: true },
  { name: 'ledger-add', path: '/trips/t-japan26/ledger?ledger=add' },
  { name: 'ledger-settle', path: '/trips/t-japan26/ledger?ledger=settle' },
  { name: 'prep', path: '/trips/t-japan26/prep', full: true },

  // The second trip is the empty-state pass: a different accent, no plan, no
  // candidates, no polls, no expenses, no notices. Every one of those was a
  // blank page at some point.
  { name: 'aegean-plan', path: '/trips/t-aegean27/plan' },
  { name: 'aegean-candidates', path: '/trips/t-aegean27/candidates' },
  { name: 'aegean-polls', path: '/trips/t-aegean27/polls' },
  { name: 'aegean-ledger', path: '/trips/t-aegean27/ledger' },
  { name: 'aegean-prep', path: '/trips/t-aegean27/prep' },
];

/** A fixed reanalysis response, so the sweep is not a load test. */
async function stubWeather(page) {
  await page.route('**open-meteo.com/**', async (route) => {
    const url = new URL(route.request().url());
    const count = (url.searchParams.get('latitude') ?? '0').split(',').length;
    const start = new Date(`${url.searchParams.get('start_date') ?? '2025-11-11'}T00:00:00Z`);
    const end = new Date(`${url.searchParams.get('end_date') ?? '2025-11-23'}T00:00:00Z`);
    const days = Math.max(1, Math.round((end - start) / 86_400_000) + 1);
    const time = Array.from({ length: days }, (_, i) =>
      new Date(start.valueOf() + i * 86_400_000).toISOString().slice(0, 10),
    );
    const daily = {
      time,
      weather_code: time.map((_, i) => [0, 2, 3, 61, 3, 2, 0][i % 7]),
      temperature_2m_max: time.map(() => 15.4),
      temperature_2m_min: time.map(() => 6.8),
      precipitation_sum: time.map(() => 1.4),
      precipitation_probability_max: time.map(() => 40),
    };
    await route.fulfill({ json: Array.from({ length: count }, () => ({ daily })) });
  });
}

const browser = await chromium.launch();
await mkdir(OUT, { recursive: true });
const failures = [];

for (const scheme of ['light', 'dark']) {
  for (const vp of VIEWPORTS) {
    const ctx = await browser.newContext({
      viewport: { width: vp.width, height: vp.height },
      colorScheme: scheme,
      deviceScaleFactor: 2,
      isMobile: vp.name === 'mobile',
      hasTouch: vp.name === 'mobile',
    });
    const page = await ctx.newPage();
    await stubWeather(page);
    page.on('pageerror', (e) => failures.push(`[pageerror] ${scheme}/${vp.name}: ${e.message}`));
    page.on('console', (m) => {
      if (m.type() === 'error') failures.push(`[console] ${scheme}/${vp.name}: ${m.text()}`);
    });

    for (const route of ROUTES) {
      await page.goto(BASE + route.path, { waitUntil: 'load' });
      await page.waitForTimeout(700);
      const file = `${OUT}/${route.name}-${vp.name}-${scheme}.png`;
      await page.screenshot({ path: file, fullPage: !!route.full });
      console.log('wrote', file);
    }
    await ctx.close();
  }
}

await browser.close();
if (failures.length) {
  console.log('\n--- PAGE ERRORS ---');
  for (const f of [...new Set(failures)]) console.log(f);
  process.exitCode = 1;
} else {
  console.log('\nno console or page errors');
}
