import { useEffect, useState } from 'react';
import { accentFrom } from '../lib/oklch';
import type { Theme } from '../lib/oklch';
import type { TripStatus } from '../api/types';

/** The active colour scheme, tracked live so a synthesised accent re-derives
    when the OS theme flips rather than staying pinned to the mount-time one. */
export function useColorScheme(): Theme {
  const [theme, setTheme] = useState<Theme>(() =>
    window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light',
  );
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => setTheme(mq.matches ? 'dark' : 'light');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);
  return theme;
}

/**
 * How strongly the day's daylight is allowed to drive the page, by trip status.
 *
 * This is the answer to "is an environment-driven page too much for a `booked`
 * itinerary read on a train?" — it is, so the effect is a dial rather than a
 * switch. A trip you are still dreaming about should feel like weather; an
 * itinerary you are navigating from should feel like a document. Same markup,
 * same code path, one number.
 */
const ENV_AMPLITUDE: Record<TripStatus, number> = {
  dreaming: 1, // a mood, not a plan — let the sky do what it likes
  planning: 0.75,
  booked: 0.3, // legible in bad light on a train; the shape still reads
  ongoing: 0.45, // enough to tell you dusk is coming while you are out in it
  done: 0.6, // a memory, warmer than a plan
};

export interface TripTheme {
  /** Synthesised accent hex, or null to keep the brand accent. */
  accent: string | null;
  amplitude: number;
}

/**
 * A trip's theme: an accent rebuilt in OKLCH from the trip's colour, and the
 * environment amplitude its status earns.
 *
 * The accent keeps only the *hue* of `Trip.accentColor` — lightness and chroma
 * are re-synthesised per theme, which is what lets an arbitrary photo-derived
 * hue clear contrast by construction instead of by audit (docs/VISUAL-DESIGN.md
 * §4). A colour too grey to have a trustworthy hue returns null, and the brand
 * accent stands.
 */
export function useTripTheme(accentColor: string | null | undefined, status: TripStatus | undefined): TripTheme {
  const theme = useColorScheme();
  return {
    accent: accentFrom(accentColor, theme),
    amplitude: status ? ENV_AMPLITUDE[status] : 1,
  };
}

/** Paint a trip's theme onto <body>, so the full viewport picks it up rather
    than just the content column. Cleaned on unmount — the neutral trip list
    must come back. */
export function useBodyTripTheme(theme: TripTheme, active: boolean): void {
  const { accent, amplitude } = theme;
  useEffect(() => {
    if (!active) return;
    const { style, classList } = document.body;
    if (accent) style.setProperty('--accent', accent);
    style.setProperty('--env-amplitude', String(amplitude));
    classList.add('trip-tinted', 'accent-scope');
    return () => {
      style.removeProperty('--accent');
      style.removeProperty('--env-amplitude');
      classList.remove('trip-tinted', 'accent-scope');
    };
  }, [accent, amplitude, active]);
}
