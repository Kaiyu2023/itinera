/**
 * Screenshot sweep for design review — not a test.
 *
 *   node e2e/shots.mjs [baseURL] [outDir]
 *
 * Walks the plan views at both breakpoints in both colour schemes, because
 * every interesting failure in this redesign is a failure in one of those four
 * quadrants and invisible in the other three.
 */
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';

const BASE = process.argv[2] ?? 'http://localhost:5173';
const OUT = process.argv[3] ?? '/tmp/shots';

const VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 900 },
  { name: 'mobile', width: 390, height: 844 },
];

const ROUTES = [
  { name: 'plan-day6', path: '/trips/t-japan26/plan?view=timeline&day=d6' },
  { name: 'plan-day1', path: '/trips/t-japan26/plan?view=timeline&day=d1' },
  { name: 'plan-day7', path: '/trips/t-japan26/plan?view=timeline&day=d7' },
  { name: 'plan-map', path: '/trips/t-japan26/plan?view=map' },
  { name: 'trips', path: '/' },
  { name: 'aegean', path: '/trips/t-aegean27/candidates' },
  { name: 'ledger', path: '/trips/t-japan26/ledger' },
  { name: 'prep', path: '/trips/t-japan26/prep' },
];

const browser = await chromium.launch();
await mkdir(OUT, { recursive: true });
const failures = [];

for (const scheme of ['light', 'dark']) {
  for (const vp of VIEWPORTS) {
    const ctx = await browser.newContext({
      viewport: { width: vp.width, height: vp.height },
      colorScheme: scheme,
      deviceScaleFactor: 2,
    });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => failures.push(`[pageerror] ${scheme}/${vp.name}: ${e.message}`));
    page.on('console', (m) => {
      if (m.type() === 'error') failures.push(`[console] ${scheme}/${vp.name}: ${m.text()}`);
    });

    for (const route of ROUTES) {
      await page.goto(BASE + route.path, { waitUntil: 'networkidle' });
      await page.waitForTimeout(350);
      const file = `${OUT}/${route.name}-${vp.name}-${scheme}.png`;
      await page.screenshot({ path: file, fullPage: route.name.startsWith('plan-day') });
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
