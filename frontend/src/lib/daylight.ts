import { formatInTz, hhmmToMin, sunTimes } from './sun';
import type { Day, PlanDetail, Stop } from '../api/types';

/** Sky colours — dawn/dusk glow between full day and night. */
const SKY = {
  night: '#353c66',
  plum: '#8a6488',
  glow: '#e8935a',
  gold: '#f4cf7a',
  day: '#dce8f2',
} as const;

export interface DaySky {
  /** Local "HH:MM" sunrise/sunset in the day's timezone. */
  rise: string;
  set: string;
  /**
   * Position of sunrise/sunset as a percentage of the day's planning window.
   * Deliberately unclamped: a negative `riseAt` means the sun was already up
   * before the window opened, and `setAt > 100` means it sets after the window
   * closes. Callers need to tell those apart from "at the very edge".
   */
  riseAt: number;
  setAt: number;
  /** Colour-stop list for a linear-gradient, ordered window-start → window-end. */
  stops: string;
}

/**
 * The day's window as a sky.
 *
 * Shared by the horizontal strip in the map panel and the vertical wash behind
 * the day canvas, so the two can never disagree about when the sun goes down.
 * Returns null when the day has no located stop to anchor on, or in polar
 * day/night where there is no rise or set to draw.
 */
export function daySky(day: Day, detail: PlanDetail, stops: Stop[]): DaySky | null {
  const anchor = stops.map((s) => detail.places.find((p) => p.id === s.placeId)).find(Boolean);
  if (!anchor) return null;
  const sun = sunTimes(day.date, anchor.lat, anchor.lng);
  if (!sun) return null;

  const rise = formatInTz(sun.sunrise, day.tz);
  const set = formatInTz(sun.sunset, day.tz);
  const windowStart = hhmmToMin(day.windowStart);
  const span = hhmmToMin(day.windowEnd) - windowStart;
  const riseAt = ((hhmmToMin(rise) - windowStart) / span) * 100;
  const setAt = ((hhmmToMin(set) - windowStart) / span) * 100;

  const c = (n: number) => Math.max(0, Math.min(100, n));
  const g: string[] = [];
  if (riseAt > -6) {
    g.push(
      `${SKY.night} ${c(riseAt - 8)}%`,
      `${SKY.plum} ${c(riseAt - 3)}%`,
      `${SKY.glow} ${c(riseAt)}%`,
      `${SKY.gold} ${c(riseAt + 3)}%`,
      `${SKY.day} ${c(riseAt + 12)}%`,
    );
  } else {
    g.push(`${SKY.day} 0%`);
  }
  if (setAt < 106) {
    g.push(
      `${SKY.day} ${c(setAt - 12)}%`,
      `${SKY.gold} ${c(setAt - 3)}%`,
      `${SKY.glow} ${c(setAt + 1)}%`,
      `${SKY.plum} ${c(setAt + 4)}%`,
      `${SKY.night} ${c(setAt + 9)}%`,
    );
  } else {
    g.push(`${SKY.day} 100%`);
  }

  return { rise, set, riseAt, setAt, stops: g.join(', ') };
}
