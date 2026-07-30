/**
 * OKLCH colour maths — enough of it to synthesise a theme accent from a photo.
 *
 * Why this exists (docs/VISUAL-DESIGN.md §4): a trip's accent is derived from
 * its cover photo, so its hue is arbitrary. A hex extracted from a photo
 * carries no lightness guarantee — pale sand from a beach shot makes white
 * glyphs vanish, near-black from a night shot makes every derived tone a
 * no-op. So we take only the *angle* from the source and rebuild lightness and
 * chroma ourselves, in a space where lightness is perceptual.
 *
 * That turns contrast from something audited per trip into a property of the
 * construction: at the lightnesses below, every one of the 360 hues clears the
 * 3:1 non-text floor in both themes (worst case 4.97:1 light, 5.88:1 dark).
 *
 * Transfer matrices are Björn Ottosson's: https://bottosson.github.io/posts/oklab/
 */

export interface Oklch {
  /** Perceptual lightness, 0–1. */
  l: number;
  /** Chroma. Unbounded in principle, but sRGB runs out well before 0.4. */
  c: number;
  /** Hue angle in degrees, 0–360. */
  h: number;
}

/* -------------------------------------------------------------------------- */
/* sRGB transfer function                                                     */

function toLinear(channel: number): number {
  const c = channel / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function encode(linear: number): number {
  const c = Math.max(0, Math.min(1, linear));
  return c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
}

/** `#rgb` or `#rrggbb` → linear-light triple. Returns null if unparseable. */
function parseHex(hex: string): [number, number, number] | null {
  const s = hex.trim().replace(/^#/, '');
  const full = s.length === 3 ? [...s].map((ch) => ch + ch).join('') : s;
  if (!/^[0-9a-fA-F]{6}$/.test(full)) return null;
  const n = parseInt(full, 16);
  return [toLinear((n >> 16) & 255), toLinear((n >> 8) & 255), toLinear(n & 255)];
}

function toHex(rgb: [number, number, number]): string {
  const part = (c: number) =>
    Math.round(255 * encode(c))
      .toString(16)
      .padStart(2, '0');
  return `#${part(rgb[0])}${part(rgb[1])}${part(rgb[2])}`;
}

/* -------------------------------------------------------------------------- */
/* OKLCH ⇄ linear sRGB                                                        */

function oklchToLinear(l: number, c: number, hDeg: number): [number, number, number] {
  const rad = (hDeg * Math.PI) / 180;
  const a = c * Math.cos(rad);
  const b = c * Math.sin(rad);

  const lc = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mc = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const sc = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;

  return [
    4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc,
    -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc,
    -0.0041960863 * lc - 0.7034186147 * mc + 1.707614701 * sc,
  ];
}

function linearToOklch(rgb: [number, number, number]): Oklch {
  const [r, g, b] = rgb;
  const lc = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const mc = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const sc = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);

  const l = 0.2104542553 * lc + 0.793617785 * mc - 0.0040720468 * sc;
  const a = 1.9779984951 * lc - 2.428592205 * mc + 0.4505937099 * sc;
  const bb = 0.0259040371 * lc + 0.7827717662 * mc - 0.808675766 * sc;

  const h = (Math.atan2(bb, a) * 180) / Math.PI;
  return { l, c: Math.hypot(a, bb), h: h < 0 ? h + 360 : h };
}

const IN_GAMUT_EPS = 1e-4;

function inGamut(rgb: [number, number, number]): boolean {
  return rgb.every((c) => c >= -IN_GAMUT_EPS && c <= 1 + IN_GAMUT_EPS);
}

/**
 * The largest chroma that still fits in sRGB at this lightness and hue, by
 * bisection.
 *
 * Clamping per hue is not optional: at L=0.52 cyan holds only C=0.094 before
 * leaving the gamut while purple holds C=0.268 — a 2.8× spread. A constant
 * chroma silently produces out-of-gamut colours for the cyan trips, which the
 * browser then clips in whatever direction it likes.
 */
export function maxChroma(l: number, h: number): number {
  let lo = 0;
  let hi = 0.4;
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    if (inGamut(oklchToLinear(l, mid, h))) lo = mid;
    else hi = mid;
  }
  return lo;
}

export function hexToOklch(hex: string): Oklch | null {
  const rgb = parseHex(hex);
  return rgb && linearToOklch(rgb);
}

/** Build a hex from OKLCH, clamping chroma into sRGB at that lightness/hue. */
export function oklchToHex(l: number, c: number, h: number): string {
  return toHex(oklchToLinear(l, Math.min(c, maxChroma(l, h)), h));
}

/* -------------------------------------------------------------------------- */
/* The accent recipe                                                          */

/**
 * Lightness the accent is pinned to in each theme. Measured ceilings, not
 * taste: `--accent-contrast` is white, and above L≈0.55 white glyphs stop
 * clearing 4.5:1 on a solid accent fill at the worst hue. 0.52 keeps headroom.
 */
export const ACCENT_L = { light: 0.52, dark: 0.72 } as const;

/** Chroma ceiling. Keeps the accent below the alarm ramp's loudest steps. */
export const ACCENT_C_MAX = 0.13;

/**
 * Below this chroma the source has no hue worth trusting — snow, fog, night,
 * black-and-white — and the extracted angle is numerical noise. Never theme
 * from a hue you cannot trust; fall back to the brand accent instead.
 */
export const HUE_CONFIDENCE_FLOOR = 0.02;

export type Theme = 'light' | 'dark';

/**
 * Synthesise the accent for a theme from a source colour.
 *
 * Only the hue survives from `source`; lightness and chroma are rebuilt, which
 * is what makes the contrast guarantee hold for an arbitrary photo. Returns
 * null when the source is missing, unparseable, or too grey to have a hue —
 * callers fall back to the brand accent.
 */
export function accentFrom(source: string | null | undefined, theme: Theme): string | null {
  if (!source) return null;
  const src = hexToOklch(source);
  if (!src || src.c < HUE_CONFIDENCE_FLOOR) return null;
  const l = ACCENT_L[theme];
  return oklchToHex(l, Math.min(ACCENT_C_MAX, maxChroma(l, src.h)), src.h);
}

/* -------------------------------------------------------------------------- */
/* Contrast — used by the tests that police the guarantee                      */

function luminance(rgb: [number, number, number]): number {
  const [r, g, b] = rgb.map((c) => Math.max(0, Math.min(1, c))) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG 2.1 contrast ratio between two hex colours. */
export function contrastRatio(a: string, b: string): number {
  const ra = parseHex(a);
  const rb = parseHex(b);
  if (!ra || !rb) return 1;
  const la = luminance(ra);
  const lb = luminance(rb);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/* -------------------------------------------------------------------------- */
/* Ink on an arbitrary fill                                                    */

/**
 * Black or white, whichever is legible on `fill`.
 *
 * The avatars are the case that forced this. Every one of them printed its
 * initial in hardcoded `#fff`, in both themes, over a colour that comes from
 * user data — so Ryuji's amber chip read **1.92:1** and Futaba's green 2.70:1,
 * at 10.9px, with the initial being the only thing distinguishing one
 * 22px circle from another in a stack.
 *
 * The 0.5 threshold is on *perceptual* lightness, not luminance: relative
 * luminance is so heavily green-weighted that it flips at the wrong place for
 * yellows and cyans, which is precisely where the failures were.
 */
export function inkOn(fill: string | null | undefined): string {
  const src = fill ? hexToOklch(fill) : null;
  if (!src) return '#ffffff';
  return src.l > 0.62 ? '#141518' : '#ffffff';
}

/** Background + a legible ink for it, for any element painted with a colour
    that came from data rather than from the palette. */
export function fillStyle(fill: string | null | undefined): { background: string; color: string } {
  const background = fill ?? '#888888';
  return { background, color: inkOn(background) };
}
