import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { contrastRatio, hexToOklch } from '../src/lib/oklch';

/**
 * The colour system's guarantees, measured in a real browser rather than
 * asserted in a document.
 *
 * Before this existed, four tokens failed the 3:1 non-text floor in one theme
 * or the other — `kind-sight` at 2.40:1 and `primary-strong` at 1.69:1 on dark
 * were effectively invisible — because the dark block redefined surfaces and
 * text but never a single hue. Nothing caught it, so nothing stopped it.
 */

/** Semantic tokens painted as non-text UI: dots, chips, rules, fills. 3:1. */
const UI_TOKENS = [
  '--color-primary',
  '--color-primary-strong',
  '--color-ok',
  '--color-tight',
  '--color-unreasonable',
  '--color-impossible',
  '--color-kind-sight',
  '--color-kind-food',
  '--color-kind-lodging',
  '--color-kind-activity',
  '--color-kind-transit',
  '--color-kind-other',
];

/** The severity ramp, in order. */
const RAMP = ['--color-tight', '--color-unreasonable', '--color-impossible'];

/** Read tokens plus the page background, all resolved to hex. */
async function readTokens(page: Page, names: string[]) {
  return page.evaluate((tokenNames) => {
    const toHex = (value: string) => {
      const probe = document.createElement('div');
      probe.style.color = value;
      document.body.append(probe);
      const [r, g, b] = getComputedStyle(probe)
        .color.match(/[\d.]+/g)!
        .map(Number);
      probe.remove();
      return `#${[r, g, b].map((c) => Math.round(c).toString(16).padStart(2, '0')).join('')}`;
    };
    const root = getComputedStyle(document.documentElement);
    const out: Record<string, string> = {
      __bg: toHex(getComputedStyle(document.body).backgroundColor),
      __text: toHex(root.getPropertyValue('--color-text')),
      __ink: toHex(root.getPropertyValue('--color-ink-on-fill')),
    };
    for (const name of tokenNames) out[name] = toHex(root.getPropertyValue(name));
    return out;
  }, names);
}

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`${scheme} theme`, () => {
    test.use({ colorScheme: scheme });

    test(`every semantic token clears 3:1 as non-text UI (${scheme})`, async ({ page }) => {
      await page.goto('/trips/t-japan26/plan?view=timeline');
      const tokens = await readTokens(page, UI_TOKENS);

      const failures: string[] = [];
      for (const name of UI_TOKENS) {
        const ratio = contrastRatio(tokens[name], tokens.__bg);
        if (ratio < 3) failures.push(`${name} ${tokens[name]} on ${tokens.__bg} = ${ratio.toFixed(2)}:1`);
      }
      expect(failures, `tokens below the 3:1 non-text floor:\n${failures.join('\n')}`).toEqual([]);
    });

    test(`severity rises monotonically in chroma (${scheme})`, async ({ page }) => {
      // The salience channel. "Louder means wrong" has to be true in the token
      // values, not just in the naming — the old scale peaked at `tight`, so
      // the middle of the scale shouted louder than its worst step.
      await page.goto('/trips/t-japan26/plan?view=timeline');
      const tokens = await readTokens(page, RAMP);
      const chroma = RAMP.map((n) => hexToOklch(tokens[n])!.c);
      for (let i = 1; i < chroma.length; i++) {
        expect(
          chroma[i],
          `${RAMP[i]} (${chroma[i].toFixed(3)}) must be more saturated than ${RAMP[i - 1]} (${chroma[i - 1].toFixed(
            3,
          )})`,
        ).toBeGreaterThan(chroma[i - 1]);
      }
    });

    test(`ink on a solid fill stays readable (${scheme})`, async ({ page }) => {
      // `.btn.approve`, `.verb.*` and `.check-box` paint a label straight onto a
      // semantic fill. That ink used to be hardcoded white, which is 2.04:1 on
      // the dark theme's --color-ok.
      await page.goto('/trips/t-japan26/plan?view=timeline');
      const tokens = await readTokens(page, ['--color-ok', '--color-impossible']);
      for (const name of ['--color-ok', '--color-impossible']) {
        expect(contrastRatio(tokens[name], tokens.__ink), `${name} vs ink`).toBeGreaterThanOrEqual(4.5);
      }
    });

    test(`the trip accent clears its floors at an arbitrary hue (${scheme})`, async ({ page }) => {
      // The accent is derived from a photo, so its hue is not ours to choose.
      // What must hold is that whatever hue arrives, the result is legible and
      // white/dark glyphs on top of it still are.
      await page.goto('/trips/t-aegean27/candidates');
      const { accent, bg, contrastInk } = await page.evaluate(() => {
        const toHex = (value: string) => {
          const probe = document.createElement('div');
          probe.style.color = value;
          document.body.append(probe);
          const [r, g, b] = getComputedStyle(probe)
            .color.match(/[\d.]+/g)!
            .map(Number);
          probe.remove();
          return `#${[r, g, b].map((c) => Math.round(c).toString(16).padStart(2, '0')).join('')}`;
        };
        const body = getComputedStyle(document.body);
        return {
          accent: toHex(body.getPropertyValue('--accent')),
          bg: toHex(body.backgroundColor),
          contrastInk: toHex(body.getPropertyValue('--accent-contrast')),
        };
      });
      expect(contrastRatio(accent, bg), `accent ${accent} on ${bg}`).toBeGreaterThanOrEqual(3);
      expect(
        contrastRatio(accent, contrastInk),
        `--accent-contrast ${contrastInk} on accent ${accent}`,
      ).toBeGreaterThanOrEqual(4.5);
    });
  });
}
