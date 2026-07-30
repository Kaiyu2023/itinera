import { useQuery } from '@tanstack/react-query';
import { conditionFromCode } from '../components/SkyGlyph';
import type { SkyCondition } from '../components/SkyGlyph';
import type { Day, PlanDetail } from '../api/types';

/**
 * Weather for the days being planned.
 *
 * Same philosophy as sunrise/sunset (`lib/sun.ts`): the environment is not the
 * backend's data. Where the sun is can be computed from a coordinate and a
 * date, and what the weather does is a public good — Open-Meteo serves both
 * forecast and 80 years of reanalysis with no key, no quota to speak of and
 * CORS open, so this costs the project nothing and the API keeps no contract
 * with us to break.
 *
 * The honest part is the distinction this draws. A forecast exists for about
 * two weeks; every trip in this app is months out, so a forecast is exactly
 * what we cannot show. What we *can* show is climate: what the same week
 * actually did in each of the last four years. That is a different claim and
 * it is labelled as one — `source: 'typical'` renders differently from
 * `source: 'forecast'`, because a planner that dresses up a multi-year median
 * as a forecast is lying to someone packing a bag.
 *
 * Every failure path returns nothing. Weather is decoration on a plan that has
 * to work on a train in a tunnel; it may never block, retry hard, or throw.
 */

/** Days ahead that Open-Meteo will actually forecast. */
const FORECAST_HORIZON_DAYS = 15;
/** Years of reanalysis to median when the date is beyond the horizon. With the
    ±3-day pad that is 28 observations a day, which is enough for a median and
    cheap enough to be polite to a free API. */
const CLIMATE_YEARS = 4;
/** Days either side of the date pulled into the sample, for a stable median. */
const CLIMATE_PAD = 3;
/** Daily total above which a sampled day counts as "wet". */
const WET_MM = 1;

const FORECAST_URL = 'https://api.open-meteo.com/v1/forecast';
const ARCHIVE_URL = 'https://archive-api.open-meteo.com/v1/archive';
const TIMEOUT_MS = 7000;

export interface DayWeather {
  /** `forecast` is a real forecast for this exact date. `typical` is the
      median of the same week across recent years — a different claim, and the
      UI must say so. */
  source: 'forecast' | 'typical';
  condition: SkyCondition;
  tempMax: number;
  tempMin: number;
  /** Chance of rain: the forecast's own figure, or the share of sampled years
      that were wet on this date. */
  wetChance: number;
  /** Inclusive year range behind a `typical` reading. */
  years?: [number, number];
}

export type TripWeather = Record<string, DayWeather>;

/* ---- date helpers (UTC throughout; these are civil dates, not instants) --- */

const DAY_MS = 86_400_000;
const iso = (d: Date) => d.toISOString().slice(0, 10);
const parse = (s: string) => new Date(`${s}T00:00:00Z`);
const shift = (d: Date, days: number) => new Date(d.valueOf() + days * DAY_MS);
/** Same month and day, `k` years earlier. Date overflow normalises, so this is
    safe across month ends and leap days. */
const yearsBack = (d: Date, k: number) => new Date(Date.UTC(d.getUTCFullYear() - k, d.getUTCMonth(), d.getUTCDate()));

/* ---- the request -------------------------------------------------------- */

/** One anchor coordinate per distinct place a day happens in. */
interface Anchor {
  lat: number;
  lng: number;
  dayIds: string[];
}

/** Days that share a coordinate to ~1km share a request. */
function anchorsFor(days: Day[], detail: PlanDetail): Anchor[] {
  const byKey = new Map<string, Anchor>();
  for (const day of days) {
    const stop = detail.stops.find((s) => s.dayId === day.id);
    const place = stop && detail.places.find((p) => p.id === stop.placeId);
    if (!place) continue;
    const lat = Math.round(place.lat * 100) / 100;
    const lng = Math.round(place.lng * 100) / 100;
    const key = `${lat},${lng}`;
    const existing = byKey.get(key);
    if (existing) existing.dayIds.push(day.id);
    else byKey.set(key, { lat, lng, dayIds: [day.id] });
  }
  return [...byKey.values()];
}

interface DailyBlock {
  time: string[];
  weather_code: number[];
  temperature_2m_max: number[];
  temperature_2m_min: number[];
  precipitation_probability_max?: number[];
  precipitation_sum?: number[];
}

/** Open-Meteo returns a bare object for one coordinate and an array for many.
    Normalise so callers only ever see the array. */
async function query(url: string, params: Record<string, string>): Promise<Array<{ daily: DailyBlock }>> {
  const qs = new URLSearchParams(params).toString();
  const res = await fetch(`${url}?${qs}`, { signal: AbortSignal.timeout(TIMEOUT_MS) });
  if (!res.ok) throw new Error(`open-meteo ${res.status}`);
  const body = await res.json();
  return Array.isArray(body) ? body : [body];
}

const coordParams = (anchors: Anchor[]) => ({
  latitude: anchors.map((a) => a.lat).join(','),
  longitude: anchors.map((a) => a.lng).join(','),
  timezone: 'auto',
});

/* ---- forecast ----------------------------------------------------------- */

async function fetchForecast(anchors: Anchor[], days: Day[], from: Date, to: Date): Promise<TripWeather> {
  const blocks = await query(FORECAST_URL, {
    ...coordParams(anchors),
    daily: 'weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max',
    start_date: iso(from),
    end_date: iso(to),
  });

  const dateOf = new Map(days.map((d) => [d.id, d.date]));
  const out: TripWeather = {};
  blocks.forEach((block, i) => {
    const anchor = anchors[i];
    if (!anchor?.dayIds.length) return;
    const { daily } = block;
    // An anchor can cover several days of the trip; each takes the row for its
    // own date.
    for (const dayId of anchor.dayIds) {
      const j = daily.time.indexOf(dateOf.get(dayId) ?? '');
      if (j < 0 || daily.temperature_2m_max[j] == null) continue;
      out[dayId] = {
        source: 'forecast',
        condition: conditionFromCode(daily.weather_code[j]),
        tempMax: Math.round(daily.temperature_2m_max[j]),
        tempMin: Math.round(daily.temperature_2m_min[j]),
        wetChance: Math.round(daily.precipitation_probability_max?.[j] ?? 0),
      };
    }
  });
  return out;
}

/* ---- climatology -------------------------------------------------------- */

interface Sample {
  code: number;
  max: number;
  min: number;
  wet: boolean;
}

/** Severity order, used only to break a tie between equally common
    conditions — when a week is half clear and half raining, a planner is
    better served by being told to pack the coat. */
const SEVERITY: SkyCondition[] = ['storm', 'snow', 'rain', 'drizzle', 'fog', 'cloud', 'partly', 'clear'];

function summarise(samples: Sample[], years: [number, number]): DayWeather | null {
  if (!samples.length) return null;
  const median = (xs: number[]) => {
    const s = [...xs].sort((a, b) => a - b);
    return s[Math.floor(s.length / 2)];
  };
  const counts = new Map<SkyCondition, number>();
  for (const s of samples) {
    const c = conditionFromCode(s.code);
    counts.set(c, (counts.get(c) ?? 0) + 1);
  }
  const condition = [...counts.entries()].sort(
    (a, b) => b[1] - a[1] || SEVERITY.indexOf(a[0]) - SEVERITY.indexOf(b[0]),
  )[0][0];

  return {
    source: 'typical',
    condition,
    tempMax: Math.round(median(samples.map((s) => s.max))),
    tempMin: Math.round(median(samples.map((s) => s.min))),
    wetChance: Math.round((samples.filter((s) => s.wet).length / samples.length) * 100),
    years,
  };
}

async function fetchTypical(anchors: Anchor[], days: Day[], from: Date, to: Date): Promise<TripWeather> {
  const dayIndex = new Map(days.map((d) => [d.id, Math.round((parse(d.date).valueOf() - from.valueOf()) / DAY_MS)]));
  const perDay = new Map<string, Sample[]>();

  // One request per year, each covering the whole trip padded either side.
  // Days line up by their offset from the range start, which is why the range
  // is rebuilt the same way for every year rather than matched on month/day —
  // that also makes a trip spanning New Year fall out for free.
  // Sequential, not a burst. Four parallel archive requests per mounted plan
  // is how you get a 429 out of a service that asks for nothing in return, and
  // a year that fails should not take the other three down with it.
  const results: Array<Array<{ daily: DailyBlock }>> = [];
  for (let k = 1; k <= CLIMATE_YEARS; k++) {
    try {
      results.push(
        await query(ARCHIVE_URL, {
          ...coordParams(anchors),
          daily: 'weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum',
          start_date: iso(shift(yearsBack(from, k), -CLIMATE_PAD)),
          end_date: iso(shift(yearsBack(to, k), CLIMATE_PAD)),
        }),
      );
    } catch {
      /* one missing year just makes the median a little coarser */
    }
  }

  for (const blocks of results) {
    blocks.forEach((block, i) => {
      const anchor = anchors[i];
      if (!anchor) return;
      const { daily } = block;
      for (const dayId of anchor.dayIds) {
        const target = dayIndex.get(dayId);
        if (target === undefined) continue;
        // Row 0 of the response is CLIMATE_PAD days before the range start, so
        // the day's own row sits at `target + CLIMATE_PAD`; take the window
        // around it.
        const centre = target + CLIMATE_PAD;
        for (let j = centre - CLIMATE_PAD; j <= centre + CLIMATE_PAD; j++) {
          if (j < 0 || j >= daily.time.length) continue;
          const max = daily.temperature_2m_max[j];
          const min = daily.temperature_2m_min[j];
          if (max == null || min == null) continue;
          const list = perDay.get(dayId) ?? [];
          list.push({ code: daily.weather_code[j] ?? 3, max, min, wet: (daily.precipitation_sum?.[j] ?? 0) >= WET_MM });
          perDay.set(dayId, list);
        }
      }
    });
  }

  const span: [number, number] = [from.getUTCFullYear() - CLIMATE_YEARS, from.getUTCFullYear() - 1];
  const out: TripWeather = {};
  for (const [dayId, samples] of perDay) {
    const summary = summarise(samples, span);
    if (summary) out[dayId] = summary;
  }
  return out;
}

/* ---- entry point -------------------------------------------------------- */

export async function fetchTripWeather(days: Day[], detail: PlanDetail, today = new Date()): Promise<TripWeather> {
  const anchors = anchorsFor(days, detail);
  if (!anchors.length || !days.length) return {};

  const dates = days.map((d) => parse(d.date)).sort((a, b) => a.valueOf() - b.valueOf());
  const from = dates[0];
  const to = dates[dates.length - 1];
  const horizon = shift(parse(iso(today)), FORECAST_HORIZON_DAYS);

  // A trip can straddle the horizon: forecast the near end, model the far end,
  // and let the forecast win where both have an answer.
  const jobs: Array<Promise<TripWeather>> = [];
  if (from <= horizon) jobs.push(fetchForecast(anchors, days, from, to < horizon ? to : horizon));
  if (to > horizon) jobs.push(fetchTypical(anchors, days, from, to));

  const settled = await Promise.allSettled(jobs);
  const merged: TripWeather = {};
  // Reverse order so `typical` lands first and `forecast` overwrites it.
  for (const result of settled.reverse()) {
    if (result.status === 'fulfilled') Object.assign(merged, result.value);
  }
  return merged;
}

/* ---- cache -------------------------------------------------------------- */

/**
 * Survives the reload, because a multi-year median for a week in November does
 * not move and this app is meant to open on roaming data. React Query's cache
 * dies with the tab, which would have every visit re-spending four archive
 * requests on an answer that was already correct.
 *
 * A real forecast gets hours, not days — that one does move.
 */
const CACHE_PREFIX = 'itinera.wx.';
const TTL_TYPICAL_MS = 7 * 24 * 3600_000;
const TTL_FORECAST_MS = 3 * 3600_000;

function readCache(key: string): TripWeather | null {
  try {
    const raw = localStorage.getItem(CACHE_PREFIX + key);
    if (!raw) return null;
    const { at, ttl, data } = JSON.parse(raw) as { at: number; ttl: number; data: TripWeather };
    return Date.now() - at > ttl ? null : data;
  } catch {
    return null;
  }
}

function writeCache(key: string, data: TripWeather) {
  const ttl = Object.values(data).some((w) => w.source === 'forecast') ? TTL_FORECAST_MS : TTL_TYPICAL_MS;
  try {
    localStorage.setItem(CACHE_PREFIX + key, JSON.stringify({ at: Date.now(), ttl, data }));
  } catch {
    /* private mode, quota, whatever — the fetch already succeeded */
  }
}

/**
 * Weather for a trip's days, or `{}`. Never throws, never retries, never
 * blocks: the plan renders identically without it.
 */
export function useTripWeather(tripId: string | undefined, days: Day[], detail: PlanDetail | undefined): TripWeather {
  const key = `${tripId}:${days.map((d) => d.id).join()}`;
  const query = useQuery({
    queryKey: ['weather', key],
    queryFn: async () => {
      const hit = readCache(key);
      if (hit) return hit;
      const fresh = await fetchTripWeather(days, detail!).catch(() => ({}));
      if (Object.keys(fresh).length) writeCache(key, fresh);
      return fresh;
    },
    enabled: !!tripId && !!detail && days.length > 0,
    staleTime: 6 * 3600_000,
    gcTime: 24 * 3600_000,
    retry: false,
    refetchOnWindowFocus: false,
  });
  return query.data ?? {};
}
