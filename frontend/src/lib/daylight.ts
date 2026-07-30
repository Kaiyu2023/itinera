import { formatInTz, hhmmToMin, sunTimes } from './sun';
import type { Day, PlanDetail, Stop } from '../api/types';

/**
 * The day's sky, as geometry.
 *
 * This module decides *when* the light changes; `tokens.css` decides what the
 * light looks like (`--sky-night` … `--sky-day`). The split matters: the two
 * themes need opposite ramps — on a cream page night is the strong band, on a
 * dark page daylight is — and a gradient with hexes compiled into it here
 * could only ever serve one of them. Emitting `var(--sky-*)` into the gradient
 * string lets the same geometry paint both.
 *
 * Shared by the horizontal strip in the map panel, the vertical wash behind
 * the day canvas, and the trip ribbon, so none of the three can disagree about
 * when the sun goes down.
 */

/** Minutes on either side of the horizon that the twilight ramp occupies. */
const DAWN = { night: -34, dusk: -16, glow: -4, gold: 10, day: 40 } as const;

export interface DaySky {
  /** Local "HH:MM" sunrise/sunset in the day's timezone. */
  rise: string;
  set: string;
  /** The same instants as minutes past local midnight — the form every
      geometric comparison actually wants. */
  riseMin: number;
  setMin: number;
  /**
   * Position of sunrise/sunset as a percentage of the day's planning window.
   * Deliberately unclamped: a negative `riseAt` means the sun was already up
   * before the window opened, and `setAt > 100` means it sets after the window
   * closes. Callers need to tell those apart from "at the very edge".
   */
  riseAt: number;
  setAt: number;
  /** Colour-stop list for a linear-gradient across the window. */
  stops: string;
}

/**
 * Gradient stops for an arbitrary slice of the day, given in minutes past
 * local midnight. Callers pass the extent they are actually painting — the day
 * canvas includes the overrun past the window's close, the ribbon paints the
 * window, the map strip paints the window — and get a ramp positioned in that
 * slice's own percentage space.
 */
export function skyGradient(riseMin: number, setMin: number, fromMin: number, spanMin: number): string {
  const at = (min: number) => ((min - fromMin) / Math.max(1, spanMin)) * 100;
  const clamp = (n: number) => Math.max(0, Math.min(100, n));

  // Built as (position, colour) pairs and then sorted, so the two horizons can
  // be written independently without either having to know whether the slice
  // it lands in overlaps the other's ramp.
  const marks: Array<[number, string]> = [];
  const ramp = (centre: number, order: Array<[number, string]>) => {
    for (const [offset, colour] of order) marks.push([at(centre + offset), colour]);
  };

  ramp(riseMin, [
    [DAWN.night, 'var(--sky-night)'],
    [DAWN.dusk, 'var(--sky-dusk)'],
    [DAWN.glow, 'var(--sky-glow)'],
    [DAWN.gold, 'var(--sky-gold)'],
    [DAWN.day, 'var(--sky-day)'],
  ]);
  // Dusk is dawn played backwards.
  ramp(setMin, [
    [-DAWN.day, 'var(--sky-day)'],
    [-DAWN.gold, 'var(--sky-gold)'],
    [-DAWN.glow, 'var(--sky-glow)'],
    [-DAWN.dusk, 'var(--sky-dusk)'],
    [-DAWN.night, 'var(--sky-night)'],
  ]);

  // Anchor both ends so a slice that contains neither horizon still gets the
  // right flat colour: before dawn and after dusk that is night, in between it
  // is daylight.
  const edge = (min: number) =>
    min < riseMin ? 'var(--sky-night)' : min > setMin ? 'var(--sky-night)' : 'var(--sky-day)';
  marks.unshift([0, edge(fromMin)]);
  marks.push([100, edge(fromMin + spanMin)]);

  return marks
    .filter(([p]) => p > -60 && p < 160)
    .sort((a, b) => a[0] - b[0])
    .map(([p, c]) => `${c} ${clamp(p).toFixed(2)}%`)
    .join(', ');
}

/**
 * The day's window as a sky. Returns null when the day has no located stop to
 * anchor on, or in polar day/night where there is no rise or set to draw.
 */
export function daySky(day: Day, detail: PlanDetail, stops: Stop[]): DaySky | null {
  const anchor = stops.map((s) => detail.places.find((p) => p.id === s.placeId)).find(Boolean);
  if (!anchor) return null;
  const sun = sunTimes(day.date, anchor.lat, anchor.lng);
  if (!sun) return null;

  const rise = formatInTz(sun.sunrise, day.tz);
  const set = formatInTz(sun.sunset, day.tz);
  const riseMin = hhmmToMin(rise);
  const setMin = hhmmToMin(set);
  const windowStart = hhmmToMin(day.windowStart);
  const span = Math.max(1, hhmmToMin(day.windowEnd) - windowStart);

  return {
    rise,
    set,
    riseMin,
    setMin,
    riseAt: ((riseMin - windowStart) / span) * 100,
    setAt: ((setMin - windowStart) / span) * 100,
    stops: skyGradient(riseMin, setMin, windowStart, span),
  };
}
