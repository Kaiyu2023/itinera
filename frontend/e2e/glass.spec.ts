import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';

/**
 * Two complaints, one of them structural.
 *
 * "In light mode the evening is not dark enough; in dark mode the daytime is
 * too dark and looks like evening." Both halves are about *composed* colour —
 * the token's alpha over the substrate it actually lands on — which is why
 * neither was visible in the token values and neither was caught by any
 * assertion here. The cause was a single blanket opacity on the wash that
 * dropped as low as 0.685 with trip status, so every alpha in tokens.css was a
 * claim about a layer nobody ever saw at full strength. The dial moved to the
 * scene; these tests pin what the wash must compose to.
 *
 * The rest is the glass pass and the full-bleed ribbon.
 */

/** sRGB relative luminance, per WCAG. */
function luminance([r, g, b]: number[]): number {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}

/**
 * What a sky token actually paints, given the surface under it and the wash's
 * own opacity. Resolved through the browser rather than parsed by hand: the
 * tokens are authored as `rgb(a b c / d)` and only the engine is authoritative
 * about what that composites to.
 */
async function composed(page: Page, token: string): Promise<number[]> {
  const rgba = await page.evaluate((name) => {
    const probe = document.createElement('div');
    probe.style.color = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    document.body.append(probe);
    const out = getComputedStyle(probe).color;
    probe.remove();
    const under = getComputedStyle(document.documentElement).getPropertyValue('--color-surface-sunken').trim();
    const p2 = document.createElement('div');
    p2.style.color = under;
    document.body.append(p2);
    const base = getComputedStyle(p2).color;
    p2.remove();
    const wash = document.querySelector('.rb-wash');
    return { out, base, opacity: wash ? Number(getComputedStyle(wash).opacity) : 1 };
  }, token);

  const nums = (s: string) => s.match(/[\d.]+/g)!.map(Number);
  const [r, g, b, a = 1] = nums(rgba.out);
  const [br, bg, bb] = nums(rgba.base);
  const k = a * rgba.opacity;
  return [r * k + br * (1 - k), g * k + bg * (1 - k), b * k + bb * (1 - k)];
}

test.describe('the sky composes to day and night', () => {
  test.use({ colorScheme: 'light' });

  test('on a cream page, night is night', async ({ page }) => {
    await page.goto('/trips/t-japan26/plan?view=timeline');
    await expect(page.locator('.rb-wash').first()).toBeVisible();

    const night = luminance(await composed(page, '--sky-night'));
    const day = luminance(await composed(page, '--sky-day'));

    // The value this replaces composed to L*≈38 — a mid slate. Anything above
    // 0.06 here is "late afternoon in bad weather", not after dark.
    expect(night).toBeLessThan(0.06);
    expect(day).toBeGreaterThan(0.55);
    // And the two halves of a day must not arrive as the same grey.
    expect((day + 0.05) / (night + 0.05)).toBeGreaterThan(5);
  });

  test('on a dark page, daylight is a lift and not another evening', async ({ page }) => {
    await page.goto('/trips/t-japan26/plan?view=timeline');
    await page.getByRole('radio', { name: 'Dark' }).click();
    await expect(page.locator('.rb-wash').first()).toBeVisible();

    const night = luminance(await composed(page, '--sky-night'));
    const day = luminance(await composed(page, '--sky-day'));
    // The painted page, not `body`'s own background — that is transparent here,
    // and a transparent box has no luminance to be darker than.
    const pageL = luminance(
      (
        await page.evaluate(() => {
          const p = document.createElement('div');
          p.style.color = getComputedStyle(document.documentElement).getPropertyValue('--color-bg').trim();
          document.body.append(p);
          const out = getComputedStyle(p).color;
          p.remove();
          return out;
        })
      )
        .match(/[\d.]+/g)!
        .map(Number),
    );

    // The ramp inverts here: day lifts off the page, night sinks below it. The
    // old numbers had day at 22% alpha, which lifted #1d1f24 to L*≈21 — the
    // same evening the night band was already showing.
    expect(day).toBeGreaterThan(pageL * 3);
    expect(night).toBeLessThan(pageL);
    expect((day + 0.05) / (night + 0.05)).toBeGreaterThan(2.5);
  });

  test('status dims the scene, never the difference between day and night', async ({ page }) => {
    // A `booked` trip is quieter, but "this stop happens after dark" is a fact
    // about the plan and the amplitude dial is not allowed to erase a fact.
    await page.goto('/trips/t-japan26/plan?view=timeline');
    // Null-tolerant: changing the phase invalidates the trip query, and the
    // ribbon is gone for a frame or two while it re-renders. The poll is what
    // waits that out — throwing inside it would only report the gap.
    const opacities = () =>
      page.evaluate(() => {
        const wash = document.querySelector('.rb-wash');
        const scene = document.querySelector('.sky-scene');
        if (!wash || !scene) return null;
        return {
          wash: Number(getComputedStyle(wash).opacity),
          scene: Number(getComputedStyle(scene).opacity),
        };
      });
    const settled = async () => {
      let v: Awaited<ReturnType<typeof opacities>> = null;
      await expect.poll(async () => (v = await opacities()) !== null).toBe(true);
      return v!;
    };

    await page.getByRole('button', { name: /Trip phase/ }).click();
    await page.getByRole('menuitem', { name: /Dreaming/ }).click();
    await expect.poll(async () => (await opacities())?.wash).toBeCloseTo(1, 2);
    const loud = await settled();

    await page.getByRole('button', { name: /Trip phase/ }).click();
    await page.getByRole('menuitem', { name: /Booked/ }).click();
    await expect.poll(async () => (await opacities())?.scene ?? loud.scene).toBeLessThan(loud.scene - 0.2);
    const quiet = await settled();

    // The scene gives up more than half its presence; the wash gives up a tenth.
    expect(quiet.scene).toBeLessThan(loud.scene * 0.7);
    expect(quiet.wash).toBeGreaterThan(0.85);
  });
});

test('the ribbon bleeds on a phone and keeps the measure on a desktop', async ({ page, isMobile }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');
  const ribbon = page.locator('.ribbon');
  await expect(ribbon).toBeVisible();

  const m = await page.evaluate(() => {
    const r = document.querySelector('.ribbon')!.getBoundingClientRect();
    const main = document.querySelector('main')!;
    const mb = main.getBoundingClientRect();
    const cs = getComputedStyle(main);
    const first = document.querySelector('.rb-day')!.getBoundingClientRect();
    return {
      left: r.left,
      right: r.right,
      win: window.innerWidth,
      docScroll: document.documentElement.scrollWidth,
      colLeft: mb.left + parseFloat(cs.paddingLeft),
      colRight: mb.right - parseFloat(cs.paddingRight),
      firstLeft: first.left,
    };
  });

  if (isMobile) {
    // Edge to edge: the column *is* the viewport here, so the page margin
    // either side of the weather is two stripes of nothing.
    expect(m.left).toBeLessThanOrEqual(0.5);
    expect(m.right).toBeGreaterThanOrEqual(m.win - 0.5);
    // The first day still starts under the page's own left margin, so the
    // ribbon reads as running *out* to the edge, not as a detached band.
    expect(m.firstLeft).toBeCloseTo(m.colLeft, 0);
  } else {
    // In the measure, where everything else on the page starts. A band running
    // the full width of a 1280px room is a stripe behind the page rather than
    // part of it, and it moves the left edge of the itinerary.
    expect(m.left).toBeCloseTo(m.colLeft, 0);
    expect(m.right).toBeCloseTo(m.colRight, 0);
    // Nothing is pushed back in, because nothing went out.
    expect(m.firstLeft).toBeCloseTo(m.left, 0);
  }
  // Neither case may put a horizontal scrollbar on the page. 100vw includes the
  // vertical scrollbar, so an ancestor has to clip; this is how that breaks.
  expect(m.docScroll).toBeLessThanOrEqual(m.win);
});

test('the day label is a pane over its own sky, and the empty hours are just sky', async ({ page }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');

  const plate = page.locator('.rb-plate').first();
  await expect(plate).toBeVisible();
  // Glass is the backdrop-filter, not the tint: without it the pane is a
  // slightly wrong surface, and text over a photo has nothing holding it up.
  await expect(plate).toHaveCSS('backdrop-filter', /blur/);
  // It sits inside the band it labels — that is the whole point of moving it
  // off the two text rows that used to bracket the band.
  const inside = await page.evaluate(() => {
    const p = document.querySelector('.rb-plate')!.getBoundingClientRect();
    const s = document.querySelector('.rb-sky')!.getBoundingClientRect();
    return p.top >= s.top && p.bottom <= s.bottom && p.left >= s.left && p.right <= s.right;
  });
  expect(inside).toBe(true);

  // Unplanned time draws nothing of its own. It used to be a dot grid, and
  // before that a barber pole; the honest picture of "nothing is happening
  // here" is the weather, showing through.
  const tail = page.locator('.dc-tail').first();
  await expect(tail).toBeVisible();
  await expect(tail).toHaveCSS('background-image', 'none');
  await expect(tail).toHaveCSS('border-top-width', '0px');
  // …but the control inside it is still a control, and it is glass.
  await expect(tail.locator('span').first()).toHaveCSS('backdrop-filter', /blur/);
});

test('the day canvas paints its sky over its own gutter and its clock on glass', async ({ page, isMobile }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');
  await expect(page.locator('.dc-sky')).toBeVisible();

  const m = await page.evaluate(() => {
    const sky = document.querySelector('.dc-sky')!.getBoundingClientRect();
    const scene = document.querySelector('.dc-scene')!.getBoundingClientRect();
    const rail = document.querySelector('.dc-rail')!;
    const railBox = rail.getBoundingClientRect();
    const canvas = document.querySelector('.daycanvas')!.getBoundingClientRect();
    const label = document.querySelector('.dc-hour i')!.getBoundingClientRect();
    const mark = document.querySelector('.dc-hz-mark')?.getBoundingClientRect() ?? null;
    const main = document.querySelector('main')!;
    const mb = main.getBoundingClientRect();
    const cs = getComputedStyle(main);
    return {
      skyLeft: sky.left,
      skyRight: sky.right,
      sceneLeft: scene.left,
      sceneRight: scene.right,
      win: window.innerWidth,
      docScroll: document.documentElement.scrollWidth,
      railLeft: railBox.left,
      railRight: railBox.right,
      canvasLeft: canvas.left,
      labelLeft: label.left,
      labelRight: label.right,
      markLeft: mark?.left ?? null,
      markRight: mark?.right ?? null,
      colLeft: mb.left + parseFloat(cs.paddingLeft),
      colRight: mb.right - parseFloat(cs.paddingRight),
      blur: getComputedStyle(rail).backdropFilter,
    };
  });

  // Whatever the sky's width is, it reaches back over the gutter — the hours
  // are printed on the same weather the stops are — and the scene travels with
  // it, or the stars would stop where the column does.
  expect(m.skyLeft).toBeLessThanOrEqual(m.railLeft + 0.5);
  expect(m.sceneLeft).toBeCloseTo(m.skyLeft, 0);
  expect(m.sceneRight).toBeCloseTo(m.skyRight, 0);
  expect(m.docScroll).toBeLessThanOrEqual(m.win);

  if (isMobile) {
    expect(m.skyLeft).toBeLessThanOrEqual(0.5);
    expect(m.skyRight).toBeGreaterThanOrEqual(m.win - 0.5);
  } else {
    // The column and its gutter, and not one pixel of the room beyond it.
    expect(m.skyLeft).toBeCloseTo(m.colLeft, 0);
    expect(m.skyRight).toBeCloseTo(m.colRight, 0);
  }

  // The rail occupies the gutter, to the left of the column and nothing else.
  expect(m.railRight).toBeLessThanOrEqual(m.canvasLeft + 0.5);
  expect(m.blur).toMatch(/blur/);
  // …and every hour is printed *centred on* it, not hung off its right edge.
  // Shrink-wrapped text let the left margin fall wherever the digits ended,
  // which put 3px of glass on one side of `07:00` and 13px on the other.
  expect(m.labelLeft).toBeCloseTo(m.railLeft, 0);
  expect(m.labelRight).toBeCloseTo(m.railRight, 0);
  // The horizon tokens share that axis: equal rail either side of them.
  if (m.markLeft !== null && m.markRight !== null) {
    expect(m.markLeft - m.railLeft).toBeCloseTo(m.railRight - m.markRight, 0);
  }
});

for (const scheme of ['light', 'dark'] as const) {
  test(`what is printed between two cards is legible against its own surface (${scheme})`, async ({ page }) => {
    // The gap row sits directly on the sky, which is a different colour at
    // every hour of every day. Two things were composited against it rather
    // than against a substrate of their own: the feasibility chips, whose
    // background was an 18% wash of their own hue, and so measured 3.52:1 in
    // the light theme and 3.43:1 in the dark — the one label out here whose job
    // is to raise an alarm. And `--color-text-muted`, which is calibrated
    // against the page (4.36:1 for the slack pill, 2.35:1 for the tail's
    // caption on glass over the night band).
    //
    // The rule this pins: anything with a surface out here has an *opaque* one,
    // and clears AA against it. Measured on tokens rather than pixels, so it
    // holds for a sky nobody has drawn yet.
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto('/trips/t-japan26/plan?view=timeline&day=d6');
    await expect(page.locator('.dc-leg .leg-chip').first()).toBeVisible();

    const labels = await page.locator('.dc-leg .leg-chip, .dc-slack').evaluateAll((els) =>
      els.map((el) => {
        const cs = getComputedStyle(el);
        const nums = (s: string) => s.match(/[\d.]+/g)!.map(Number);
        return {
          what: `${el.className} — ${el.textContent!.slice(0, 24)}`,
          ink: nums(cs.color),
          surface: nums(cs.backgroundColor),
        };
      }),
    );
    // Day 6 has three legs, one of them `tight`, plus two slack readouts.
    expect(labels.length).toBe(5);

    const thin: string[] = [];
    for (const l of labels) {
      expect(l.surface[3] ?? 1, `${l.what} is standing on a translucent surface`).toBe(1);
      const [a, b] = [luminance(l.ink), luminance(l.surface)].sort((x, y) => y - x);
      const ratio = (a + 0.05) / (b + 0.05);
      if (ratio < 4.5) thin.push(`${l.what} = ${ratio.toFixed(2)}:1`);
    }
    expect(thin, `below AA on their own surface:\n${thin.join('\n')}`).toEqual([]);

    // The unplanned pill is glass on purpose — it is floating over a photograph
    // of the weather — so it cannot have a surface of its own to be measured
    // against. It buys its legibility the other way, with full-strength ink in
    // both of its two lines; the caption stays quiet by being smaller and
    // unbolded, not by being grey.
    const pill = await page
      .locator('.dc-tail')
      .first()
      .evaluate((el) => ({
        duration: getComputedStyle(el.querySelector('b')!).color,
        caption: getComputedStyle(el.querySelector('em')!).color,
        text: getComputedStyle(document.documentElement).getPropertyValue('color'),
        body: getComputedStyle(document.body).color,
      }));
    expect(pill.caption).toBe(pill.duration);
    expect(pill.caption).toBe(pill.body);
  });
}

test('the ribbon is panned, not scrollbarred', async ({ page, isMobile }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');
  const track = page.locator('.rb-track');
  await expect(track).toBeVisible();
  await expect(track).toHaveCSS('scrollbar-width', 'none');

  // The fade means "there is more this way", so at rest the front of the trip
  // is not faded. Inside the column the first day starts at the track's own
  // edge, where an unconditional mask would dissolve Monday to announce days
  // that do not exist.
  await expect(track).not.toHaveClass(/more-back/);
  await expect(track).toHaveClass(/more-fwd/);

  const back = page.getByRole('button', { name: 'Earlier days' });
  const fwd = page.getByRole('button', { name: 'Later days' });

  if (isMobile) {
    // A finger pans the thing directly and flings it, which no chevron
    // improves on — there they would just be two objects sitting on top of the
    // first and last day.
    await expect(back).toBeHidden();
    await expect(fwd).toBeHidden();
    return;
  }
  // Disabled rather than hidden, so the row does not change width and a
  // keyboard user is not offered a direction that does not exist.
  await expect(back).toBeDisabled();
  await expect(fwd).toBeEnabled();

  await fwd.click();
  await expect.poll(() => track.evaluate((el) => el.scrollLeft)).toBeGreaterThan(100);
  await expect(back).toBeEnabled();

  // Dragging pans it — and the day under the cursor when the drag stops is not
  // a day you asked for.
  const before = await page.locator('.rb-day.active').getAttribute('data-day');
  const at = await track.boundingBox();
  const start = await track.evaluate((el) => el.scrollLeft);
  await page.mouse.move(at!.x + at!.width / 2, at!.y + 50);
  await page.mouse.down();
  for (let i = 1; i <= 8; i++) await page.mouse.move(at!.x + at!.width / 2 + i * 25, at!.y + 50);
  await page.mouse.up();

  await expect.poll(() => track.evaluate((el) => el.scrollLeft)).toBeLessThan(start - 100);
  expect(await page.locator('.rb-day.active').getAttribute('data-day')).toBe(before);
});

test('the top bar lets the page through', async ({ page }) => {
  await page.goto('/trips/t-japan26/plan?view=timeline');
  const bar = page.locator('header.topbar');
  await expect(bar).toBeVisible();
  await expect(bar).toHaveCSS('backdrop-filter', /blur/);
  await expect(bar).toHaveCSS('position', 'sticky');
  // Translucent, or the blur has nothing to do.
  const alpha = await bar.evaluate((el) => {
    const bg = getComputedStyle(el).backgroundColor;
    const n = bg.match(/[\d.]+/g)!.map(Number);
    return n.length > 3 ? n[3] : 1;
  });
  expect(alpha).toBeLessThan(0.95);
});
