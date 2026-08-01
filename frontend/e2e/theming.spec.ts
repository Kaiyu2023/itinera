import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { accentFrom, contrastRatio, hexToOklch } from '../src/lib/oklch';

/** The per-trip theming system: every emphasis-role control reads the semantic
    `--accent` token, which TripLayout overrides on <body> from Trip.accentColor.
    The Aegean trip's blue is the proof — before the migration, controls were
    pinned to brand tokens and stayed vermilion on every trip.

    `--accent` is no longer the trip's colour verbatim. A trip accent is meant to
    come from the trip's cover photo, so its hue is arbitrary, and an arbitrary
    hex carries no lightness guarantee — pale sand makes white glyphs vanish.
    The frontend therefore keeps only the *hue* and rebuilds lightness/chroma in
    OKLCH (src/lib/oklch.ts), which turns contrast from something audited per
    trip into a property of the construction.

    So these tests assert the property, not a literal: the painted accent must
    carry the trip's hue and must clear the contrast floor. Pinning the hex
    would only re-assert the bug. */

const SOURCE = { aegean: '#3e7fa8', japan: '#d97b4f' };
const PAGE_BG = '#fbfaf8'; // --color-bg, light theme

const accentOf = (page: Page) =>
  page.evaluate(() => getComputedStyle(document.body).getPropertyValue('--accent').trim());

/** Compare a painted accent against the trip's source colour. */
function expectDerivedFrom(painted: string, source: string) {
  const got = hexToOklch(painted);
  const want = hexToOklch(source);
  expect(got, `${painted} should be a parseable colour`).not.toBeNull();
  expect(want).not.toBeNull();
  // The hue is the part that comes from the trip.
  expect(Math.abs(got!.h - want!.h), `hue of ${painted} should match ${source}`).toBeLessThan(1.5);
  // The lightness is the part we rebuild, and the reason the guarantee holds.
  expect(got!.l).toBeCloseTo(0.52, 2);
  expect(got!.c).toBeLessThanOrEqual(0.132);
  expect(contrastRatio(painted, PAGE_BG)).toBeGreaterThanOrEqual(3);
}

test('a blue trip themes its controls blue, not brand vermilion', async ({ page }) => {
  await page.goto('/trips/t-aegean27/candidates');
  // TripLayout stamps the accent once the trip loads — poll, don't sample.
  await expect.poll(() => accentOf(page)).toBe(accentFrom(SOURCE.aegean, 'light'));
  expectDerivedFrom(await accentOf(page), SOURCE.aegean);

  // A solid-accent control resolves to the trip's blue end-to-end.
  const pitch = page.getByRole('button', { name: /Add an idea/ });
  await expect(pitch).toBeVisible();
  const painted = await pitch.evaluate((el) => getComputedStyle(el).backgroundColor);
  const expected = await page.evaluate((hex) => {
    const probe = document.createElement('div');
    probe.style.background = hex;
    document.body.append(probe);
    const out = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return out;
  }, accentFrom(SOURCE.aegean, 'light')!);
  expect(painted).toBe(expected);
});

test('the Japan trip and the trip list resolve to vermilion', async ({ page }) => {
  await page.goto('/trips/t-japan26/candidates');
  await expect.poll(() => accentOf(page)).toBe(accentFrom(SOURCE.japan, 'light'));
  expectDerivedFrom(await accentOf(page), SOURCE.japan);

  // Outside any trip the knob falls back to the brand accent, which is the same
  // hue synthesised at the same lightness — the palette input in tokens.css.
  await page.goto('/');
  await expect(page.locator('body')).not.toHaveClass(/trip-tinted/);
  expectDerivedFrom(await accentOf(page), SOURCE.japan);
});

test('a colour too grey to have a hue falls back instead of theming from noise', async ({ page }) => {
  // Snow, fog, night and black-and-white photos yield a chroma near zero, where
  // the extracted hue is numerical noise. The recipe must refuse those.
  for (const grey of ['#ffffff', '#808080', '#1a1a1a']) {
    expect(accentFrom(grey, 'light'), `${grey} has no hue worth trusting`).toBeNull();
  }
  // And the brand accent still stands when it does.
  await page.goto('/');
  expect(await accentOf(page)).toBe(accentFrom(SOURCE.japan, 'light'));
});

test('derived washes re-derive under a trip accent override', async ({ page }) => {
  // Regression: var() inside a custom property resolves where the property is
  // DEFINED — deriving --accent-soft only at :root freezes it to vermilion no
  // matter what --accent becomes. On the Aegean trip the selected-filter wash
  // must be the trip blue, which only holds if the family re-derives in scope.
  //
  // The probe used to be the ledger's selected filter chip. The Aegean trip is
  // the *empty* fixture — no plan, no polls, no expenses — and once the ledger
  // grew a real first-run state, the filter bar stopped rendering there at all.
  // It has to stay this trip, though: Japan's photo hue and the brand accent are
  // the same, so a frozen :root derivation would pass on Japan by coincidence.
  // The trip-phase menu is the one accent-soft surface every trip has, empty or
  // not. It is portalled to <body>, which is also where `accent-scope` lands —
  // if that ever stops being true this test fails first, which is the right
  // place for it to fail.
  await page.goto('/trips/t-aegean27/ledger');
  await page.getByRole('button', { name: /Trip phase/ }).click();
  const chip = page.locator('.status-menu .sm-now').first();
  await expect(chip).toBeVisible();
  // The probe is built from whatever --accent actually resolved to, so this
  // keeps policing the 12% derivation rule without re-pinning the hex.
  const { fill, probe } = await chip.evaluate((el) => {
    const accent = getComputedStyle(document.body).getPropertyValue('--accent').trim();
    const p = document.createElement('div');
    p.style.background = `color-mix(in srgb, ${accent} 12%, transparent)`;
    document.body.append(p);
    const out = { fill: getComputedStyle(el).backgroundColor, probe: getComputedStyle(p).backgroundColor };
    p.remove();
    return out;
  });
  expect(fill).toBe(probe);
});

test('poll vote highlight follows the trip accent', async ({ page }) => {
  // .opt.mine was one of the rules pinned to dusk-blue --color-primary.
  await page.goto('/trips/t-japan26/polls');
  const opt = page.locator('.opt:has(.fill)').first();
  await expect(opt).toBeVisible();
  // color-mix() serialization varies by engine, so compare against a probe
  // element given the expected mix with the resolved accent substituted in.
  const { fill, probe } = await opt.locator('.fill').evaluate((el) => {
    const accent = getComputedStyle(document.body).getPropertyValue('--accent').trim();
    const p = document.createElement('div');
    p.style.background = `color-mix(in srgb, ${accent} 12%, transparent)`;
    document.body.append(p);
    const out = { fill: getComputedStyle(el).backgroundColor, probe: getComputedStyle(p).backgroundColor };
    p.remove();
    return out;
  });
  expect(fill).toBe(probe);
});
