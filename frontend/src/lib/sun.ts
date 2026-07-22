/**
 * Sunrise/sunset, computed client-side — daylight is astronomy, not an API
 * ($0 constraint). Standard sunrise-equation implementation (the same core
 * as NOAA's calculator and suncalc), accurate to a couple of minutes, which
 * is plenty for a daylight strip.
 */

const RAD = Math.PI / 180;
const DAY_MS = 86_400_000;
const J1970 = 2_440_588; // Julian day of the Unix epoch
const J2000 = 2_451_545; // Julian day of 2000-01-01 12:00 TT
const OBLIQUITY = RAD * 23.4397;
const SUN_ALTITUDE = RAD * -0.833; // horizon, corrected for refraction + disc size

function toDays(date: Date): number {
  return date.valueOf() / DAY_MS - 0.5 + J1970 - J2000;
}

function fromJulian(j: number): Date {
  return new Date((j + 0.5 - J1970) * DAY_MS);
}

function solarMeanAnomaly(d: number): number {
  return RAD * (357.5291 + 0.98560028 * d);
}

function eclipticLongitude(M: number): number {
  const center = RAD * (1.9148 * Math.sin(M) + 0.02 * Math.sin(2 * M) + 0.0003 * Math.sin(3 * M));
  const perihelion = RAD * 102.9372;
  return M + center + perihelion + Math.PI;
}

export interface SunTimes {
  sunrise: Date;
  sunset: Date;
}

/**
 * Sun times for a civil date (`YYYY-MM-DD`) at a coordinate, as UTC instants.
 * Returns null in polar day/night (no rise/set) — callers should hide the
 * daylight strip rather than invent times.
 */
export function sunTimes(date: string, lat: number, lng: number): SunTimes | null {
  const lw = RAD * -lng;
  const phi = RAD * lat;
  const d = toDays(new Date(`${date}T12:00:00Z`));

  const cycle = Math.round(d - 0.0009 - lw / (2 * Math.PI));
  const approxNoon = 0.0009 + lw / (2 * Math.PI) + cycle;
  const M = solarMeanAnomaly(approxNoon);
  const L = eclipticLongitude(M);
  const declination = Math.asin(Math.sin(L) * Math.sin(OBLIQUITY));
  const jNoon = J2000 + approxNoon + 0.0053 * Math.sin(M) - 0.0069 * Math.sin(2 * L);

  const cosH =
    (Math.sin(SUN_ALTITUDE) - Math.sin(phi) * Math.sin(declination)) / (Math.cos(phi) * Math.cos(declination));
  if (cosH < -1 || cosH > 1) return null; // midnight sun / polar night

  const w = Math.acos(cosH);
  const jSet = J2000 + 0.0009 + (w + lw) / (2 * Math.PI) + cycle + 0.0053 * Math.sin(M) - 0.0069 * Math.sin(2 * L);
  const jRise = jNoon - (jSet - jNoon);
  return { sunrise: fromJulian(jRise), sunset: fromJulian(jSet) };
}

/** "HH:MM" wall-clock rendering of an instant in an IANA timezone. */
export function formatInTz(instant: Date, tz: string): string {
  return new Intl.DateTimeFormat('en-GB', {
    timeZone: tz,
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(instant);
}

/** Minutes since midnight for an "HH:MM" string. */
export function hhmmToMin(hhmm: string): number {
  const [h, m] = hhmm.split(':').map(Number);
  return h * 60 + m;
}
