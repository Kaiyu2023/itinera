import type { LngLat, LngLatBounds, MapMarker, MapRoute } from '../map/MapRenderer';
import { KIND_GLYPH_PATH } from '../components/KindGlyph';
import { PLACE_KIND_STOP_KIND } from './planShared';
import { translate } from '../i18n';
import type { UiLocale } from '../i18n';
import type { CandidateWithPlace, Day, DayFeasibility, Feasibility, Leg, Place, PlanDetail, Stop } from '../api/types';

const KIND_KEY = {
  visit: 'plan.kind.visit',
  meal: 'plan.kind.meal',
  lodging: 'plan.kind.lodging',
  activity: 'plan.kind.activity',
  transit: 'plan.kind.transit',
} as const;
const FEASIBILITY_KEY = {
  ok: 'plan.feasibility.ok',
  tight: 'plan.feasibility.tight',
  unreasonable: 'plan.feasibility.unreasonable',
  impossible: 'plan.feasibility.impossible',
} as const;
const tr = (locale: UiLocale, key: Parameters<typeof translate>[1], values?: Parameters<typeof translate>[2]) =>
  translate(locale, key, values);

/**
 * Pure map geometry — the marker/route/bounds builders shared by the desktop
 * map shell, the mobile map overlay, and the add-stop composer's embedded map.
 * Everything here is a plain function of plan data (no React, no side effects)
 * so it can be memoised at any call site and reused without duplicating logic.
 *
 * What the map paints, and why.
 *
 * The map was the last surface in the app still colouring by stop kind — a
 * green pin for an activity, an amber one for a meal — while the rest of the
 * plan had already given that channel over to feasibility. Two systems, one
 * ink: an amber pin meant "restaurant" here and "this leg is tight" three
 * inches away in the panel. So the map follows the rule now:
 *
 *   fill = the alarm (feasibility of the stop's legs, neutral when fine)
 *   glyph = the kind (the same five paths the timeline draws)
 *   number = the sequence, on a badge at the pin's shoulder
 *   route = per leg, in that leg's own verdict
 *
 * The route is the part only a map can say. The timeline can tell you a leg is
 * unreasonable; only the map can show you it is unreasonable *because it
 * crosses the city twice*.
 */

/** The alarm ramp, as pin fills and route strokes. `ok` is not in it: a leg
    that is fine says nothing, exactly as the timeline's verdict badges do. */
const FEASIBILITY_COLOR: Record<Exclude<Feasibility, 'ok'>, string> = {
  tight: 'var(--color-tight)',
  unreasonable: 'var(--color-unreasonable)',
  impossible: 'var(--color-impossible)',
};

/** Ink for anything with no verdict to report — declared in map.css so the
    basemap it sits on and the pin on top of it stay in the same theme. */
const NEUTRAL = 'var(--mmr-neutral)';

const SEVERITY: Record<Feasibility, number> = { ok: 0, tight: 1, unreasonable: 2, impossible: 3 };

/** How loudly a stop should read: the worst of the legs that touch it. A stop
    you can only just reach and cannot leave in time is not a calm stop.
    Exported because the map panel's rail dots answer the same question, and a
    row that disagreed with its own pin would be worse than no colour at all. */
export function stopAlarm(stop: Stop, next: Stop | undefined, legs: Leg[]): Feasibility {
  let worst: Feasibility = 'ok';
  for (const leg of legs) {
    if (leg.toStopId !== stop.id && (!next || leg.toStopId !== next.id)) continue;
    if (SEVERITY[leg.feasibility] > SEVERITY[worst]) worst = leg.feasibility;
  }
  return worst;
}

function alarmColor(f: Feasibility): string {
  return f === 'ok' ? NEUTRAL : FEASIBILITY_COLOR[f];
}

export interface EdgePad {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export const DESKTOP_PAD: EdgePad = { top: 0.22, right: 0.22, bottom: 0.22, left: 0.22 };
/** Mobile. It used to carry a 1.15 bottom pad to dodge the bottom sheet, which
    is the wrong tool twice over: a fraction of the *geographic* span cannot
    know how tall the sheet is, and it distorted the frame for the trip view,
    which used no allowance at all and squeezed seven days into a 120px strip
    with half of it behind the sheet. Chrome is measured in pixels now and
    handed to fitBounds per edge; this is only breathing room. */
export const SHEET_PAD: EdgePad = { top: 0.12, right: 0.14, bottom: 0.12, left: 0.14 };
export const TRIP_PAD: EdgePad = { top: 0.14, right: 0.14, bottom: 0.14, left: 0.14 };
/** The composer's embedded map — even padding, a touch generous so pins breathe. */
export const EMBED_PAD: EdgePad = { top: 0.26, right: 0.26, bottom: 0.26, left: 0.26 };

export function padBounds(points: LngLat[], pad: EdgePad): LngLatBounds {
  let west = Infinity;
  let east = -Infinity;
  let south = Infinity;
  let north = -Infinity;
  for (const p of points) {
    west = Math.min(west, p.lng);
    east = Math.max(east, p.lng);
    south = Math.min(south, p.lat);
    north = Math.max(north, p.lat);
  }
  if (!Number.isFinite(west)) {
    // No points at all — fall back to a Tokyo-ish frame so the map still draws.
    west = 139.7;
    east = 139.8;
    south = 35.65;
    north = 35.72;
  }
  const MIN_SPAN = 0.014; // never zoom past neighbourhood level
  if (east - west < MIN_SPAN) {
    const c = (east + west) / 2;
    west = c - MIN_SPAN / 2;
    east = c + MIN_SPAN / 2;
  }
  if (north - south < MIN_SPAN) {
    const c = (north + south) / 2;
    south = c - MIN_SPAN / 2;
    north = c + MIN_SPAN / 2;
  }
  const spanLng = east - west;
  const spanLat = north - south;
  return {
    west: west - spanLng * pad.left,
    east: east + spanLng * pad.right,
    south: south - spanLat * pad.bottom,
    north: north + spanLat * pad.top,
  };
}

export function inBounds(p: { lng: number; lat: number }, b: LngLatBounds): boolean {
  return p.lng >= b.west && p.lng <= b.east && p.lat >= b.south && p.lat <= b.north;
}

export function dayStopsOf(detail: PlanDetail, dayId: string): Stop[] {
  return detail.stops.filter((s) => s.dayId === dayId).sort((a, b) => a.seq - b.seq);
}

/** The lodging carried over from earlier days — where this day's morning starts. */
function carriedHome(detail: PlanDetail, days: Day[], dayIndex: number, placeById: Map<string, Place>): Place | null {
  let home: Place | null = null;
  for (let i = 0; i < dayIndex; i++) {
    for (const s of dayStopsOf(detail, days[i].id)) {
      if (s.stopKind === 'lodging') home = placeById.get(s.placeId) ?? home;
    }
  }
  return home;
}

export interface DayGeo {
  day: Day;
  stops: Stop[];
  placeById: Map<string, Place>;
  feasibility: DayFeasibility | undefined;
  home: Place | null; // shown only when near the day's action
  candidates: CandidateWithPlace[]; // shortlisted, inside this day's frame
  bounds: LngLatBounds;
  legs: Leg[];
}

export function buildDayGeo(
  detail: PlanDetail,
  days: Day[],
  day: Day,
  candidates: CandidateWithPlace[],
  pad: EdgePad,
): DayGeo {
  const placeById = new Map(detail.places.map((p) => [p.id, p]));
  const stops = dayStopsOf(detail, day.id);
  const pts: LngLat[] = [];
  for (const s of stops) {
    const p = placeById.get(s.placeId);
    if (p) pts.push({ lng: p.lng, lat: p.lat });
  }
  const cLng = pts.reduce((a, p) => a + p.lng, 0) / Math.max(1, pts.length);
  const cLat = pts.reduce((a, p) => a + p.lat, 0) / Math.max(1, pts.length);
  let home = carriedHome(detail, days, days.indexOf(day), placeById);
  if (home && (Math.abs(home.lng - cLng) > 0.22 || Math.abs(home.lat - cLat) > 0.22)) home = null;
  if (home) pts.push({ lng: home.lng, lat: home.lat });
  const bounds = padBounds(pts, pad);
  const dayCands = candidates.filter(
    (c) => c.status === 'shortlisted' && inBounds(c.place, bounds) && !stops.some((s) => s.placeId === c.placeId),
  );
  // Tag declutter used to happen here, comparing stop-to-stop *degrees* and
  // ignoring home / candidate / city / chip markers entirely — which is how a
  // suppressed pin kept its neighbour's name and the home tag rendered as
  // "acery Shinjuku" with 52px sliced off the frame. It belongs to the
  // renderer now, where the real tag boxes and the frame edges are knowable
  // (MockMapRenderer.renderMarkers step 4).
  const stopIds = new Set(stops.map((s) => s.id));
  const legs = detail.legs.filter((l) => stopIds.has(l.toStopId));
  return {
    day,
    stops,
    placeById,
    feasibility: detail.dayFeasibility.find((f) => f.dayId === day.id),
    home,
    candidates: dayCands,
    bounds,
    legs,
  };
}

/** While the add-stop composer is docked, candidate rings become live: the one
    selected in the composer wears the selected-marker treatment, and any can be
    clicked to select it there. */
export interface CandidatePick {
  interactive: boolean;
  selectedId: string | null;
}

export function dayMarkers(
  geo: DayGeo,
  selectedStopId: string | null,
  showCandidates: boolean,
  locale: UiLocale,
  candidatePick?: CandidatePick,
): MapMarker[] {
  const markers: MapMarker[] = [];
  if (geo.home) {
    markers.push({
      id: 'home',
      position: { lng: geo.home.lng, lat: geo.home.lat },
      variant: 'home',
      // Neutral: last night's lodging carries no verdict for today.
      color: NEUTRAL,
      glyphPath: KIND_GLYPH_PATH.lodging,
      tag: geo.home.name,
      tagPlacement: 'left',
      ariaLabel: tr(locale, 'plan.map.marker.stayingAt', { place: geo.home.name }),
      interactive: false,
    });
  }
  if (showCandidates) {
    for (const c of geo.candidates) {
      markers.push({
        id: `cand:${c.id}`,
        position: { lng: c.place.lng, lat: c.place.lat },
        // No colour at all: a candidate has no legs, so it has no verdict, and
        // it is not going to spend the alarm channel on being a restaurant.
        variant: 'candidate',
        tag: tr(locale, 'plan.map.marker.ideaTag', { place: c.place.name }),
        ariaLabel: tr(locale, 'plan.map.marker.idea', {
          place: c.place.name,
          kind: tr(locale, KIND_KEY[PLACE_KIND_STOP_KIND[c.place.kind]]),
        }),
        interactive: candidatePick?.interactive ?? false,
        selected: candidatePick?.selectedId === c.id,
      });
    }
  }
  geo.stops.forEach((s, i) => {
    const p = geo.placeById.get(s.placeId);
    if (!p) return;
    const alarm = stopAlarm(s, geo.stops[i + 1], geo.legs);
    markers.push({
      id: `stop:${s.id}`,
      position: { lng: p.lng, lat: p.lat },
      variant: 'stop',
      color: alarmColor(alarm),
      glyphPath: KIND_GLYPH_PATH[s.stopKind],
      seq: i + 1,
      // the selected stop's popover/card replaces its name tag
      tag: s.id === selectedStopId ? undefined : p.name,
      ariaLabel:
        alarm === 'ok'
          ? tr(locale, 'plan.map.marker.stop', {
              stop: i + 1,
              place: p.name,
              kind: tr(locale, KIND_KEY[s.stopKind]),
            })
          : tr(locale, 'plan.map.marker.stopAlarm', {
              stop: i + 1,
              place: p.name,
              kind: tr(locale, KIND_KEY[s.stopKind]),
              alarm: tr(locale, FEASIBILITY_KEY[alarm]),
            }),
      selected: s.id === selectedStopId,
    });
  });
  return markers;
}

/** The spur from last night's lodging to the day's first stop.
    It used to be #8a8577 — against the basemap's #b8b2a4 dashed rail lines that
    is the same mark in the same ink, so "where you slept" read as a train line.
    The accent settles it: nothing the basemap draws is ever the trip's colour.
    The dash pattern stays the app's standard `2 7`; it is the hue and the
    2.5px weight that separate this from the composer's proposed legs. */
function homeSpur(geo: DayGeo): MapRoute | null {
  const first = geo.stops[0] ? geo.placeById.get(geo.stops[0].placeId) : undefined;
  if (!geo.home || !first || geo.stops[0].placeId === geo.home.id) return null;
  return {
    id: 'spur',
    points: [
      { lng: geo.home.lng, lat: geo.home.lat },
      { lng: first.lng, lat: first.lat },
    ],
    color: 'var(--accent)',
    dashed: true,
    width: 2.5,
  };
}

/**
 * One polyline per leg, each in that leg's own verdict.
 *
 * This is the whole reason to look at a plan on a map rather than in a list:
 * the timeline can tell you a leg is unreasonable, but only the map can show
 * you that it is unreasonable because it doubles back across the city. A leg
 * that is fine stays in the trip accent and says nothing.
 */
export function dayRoutes(geo: DayGeo): MapRoute[] {
  const routes: MapRoute[] = [];
  const spur = homeSpur(geo);
  if (spur) routes.push(spur);
  for (let i = 1; i < geo.stops.length; i++) {
    const from = geo.placeById.get(geo.stops[i - 1].placeId);
    const to = geo.placeById.get(geo.stops[i].placeId);
    if (!from || !to) continue;
    const verdict = geo.legs.find((l) => l.toStopId === geo.stops[i].id)?.feasibility ?? 'ok';
    routes.push({
      id: `leg:${geo.stops[i].id}`,
      points: [
        { lng: from.lng, lat: from.lat },
        { lng: to.lng, lat: to.lat },
      ],
      color: verdict === 'ok' ? 'var(--accent)' : FEASIBILITY_COLOR[verdict],
      // Louder means wrong in weight as well as in chroma.
      width: verdict === 'ok' ? 4 : 5.5,
    });
  }
  return routes;
}

/** The day's route re-drawn with a not-yet-added stop spliced in at `seq`
    (1-based). The two legs into/out of the inserted point render DASHED in the
    trip accent; the untouched legs keep their solid style, and the direct leg
    the new stop displaces simply isn't drawn. The home spur is left as-is. */
export function proposedDayRoutes(geo: DayGeo, insertAt: LngLat, seq: number): MapRoute[] {
  const pts: LngLat[] = [];
  for (const s of geo.stops) {
    const p = geo.placeById.get(s.placeId);
    if (p) pts.push({ lng: p.lng, lat: p.lat });
  }
  const routes: MapRoute[] = [];
  const spur = homeSpur(geo);
  if (spur) routes.push(spur);
  const i = Math.max(0, Math.min(pts.length, Math.round(seq) - 1));
  const before = pts.slice(0, i);
  const after = pts.slice(i);
  const prev = before[before.length - 1];
  const next = after[0];
  if (before.length >= 2) routes.push({ id: 'day-before', points: before, color: 'var(--accent)' });
  if (after.length >= 2) routes.push({ id: 'day-after', points: after, color: 'var(--accent)' });
  if (prev) routes.push({ id: 'ins-in', points: [prev, insertAt], color: 'var(--accent)', dashed: true });
  if (next) routes.push({ id: 'ins-out', points: [insertAt, next], color: 'var(--accent)', dashed: true });
  return routes;
}

/** A distinct pin at the spot a proposed stop would land, wearing the selected
    treatment and labelled with its new (1-based) sequence number. Accent, not
    the alarm ramp: it is a proposal, and it has no legs to judge yet. */
export function proposedStopMarker(insertAt: LngLat, seq: number, locale: UiLocale): MapMarker {
  return {
    id: 'proposed',
    position: insertAt,
    variant: 'stop',
    color: 'var(--accent)',
    seq,
    label: '+',
    tag: tr(locale, 'plan.map.marker.newStop'),
    tagPlacement: 'above',
    selected: true,
    interactive: false,
  };
}

/** Pins for a set of place-search hits, keyed `sr:<placeId>` so click routing
    can tell them apart from stops and candidates. Neutral: a search hit is not
    in the plan, so it has nothing to warn about. */
export function searchResultMarkers(results: Place[], selectedId: string | null, locale: UiLocale): MapMarker[] {
  return results.map((p) => ({
    id: `sr:${p.id}`,
    position: { lng: p.lng, lat: p.lat },
    variant: 'search-result' as const,
    color: NEUTRAL,
    tag: p.name,
    ariaLabel: tr(locale, 'plan.map.marker.searchResult', {
      place: p.name,
      kind: tr(locale, KIND_KEY[PLACE_KIND_STOP_KIND[p.kind]]),
    }),
    selected: p.id === selectedId,
  }));
}

export interface TripGeo {
  routes: MapRoute[];
  markers: MapMarker[]; // everything except candidates (toggle-independent)
  candidateMarkers: MapMarker[];
  bounds: LngLatBounds;
}

export function buildTripGeo(
  detail: PlanDetail,
  days: Day[],
  candidates: CandidateWithPlace[],
  locale: UiLocale,
): TripGeo {
  const placeById = new Map(detail.places.map((p) => [p.id, p]));
  const routes: MapRoute[] = [];
  const markers: MapMarker[] = [];
  const allPts: LngLat[] = [];

  /**
   * One route for the whole trip, drawn leg by leg in each leg's own verdict,
   * with a numbered bead where each day starts.
   *
   * There used to be seven polylines, one per day, each in a colour from a
   * seven-hue wheel. Every one of them ran the same Tōkaidō corridor and each
   * was drawn with an opaque 7.5px halo *over* its predecessor, so the overview
   * showed exactly one line — whichever day happened to be drawn last, in a
   * purple that meant "day 5" and nothing else. Worse, the same day changed
   * colour when you zoomed: accent in day view, a wheel hue in trip view.
   * Which day is where is now the beads' job, and colour goes back to meaning
   * what it means everywhere else in this app.
   */
  let prev: LngLat | null = null;
  let home: Place | null = null;
  for (let i = 0; i < days.length; i++) {
    const day = days[i];
    const stops = dayStopsOf(detail, day.id);
    const verdict = detail.dayFeasibility.find((f) => f.dayId === day.id)?.feasibility ?? 'ok';
    if (home) {
      // The night before continues into this morning.
      const at = { lng: home.lng, lat: home.lat };
      if (prev) routes.push(tripLeg(`carry:${day.id}`, prev, at, 'ok'));
      prev = at;
      allPts.push(at);
    }
    let bead: LngLat | null = null;
    for (const s of stops) {
      const p = placeById.get(s.placeId);
      if (!p) continue;
      const at = { lng: p.lng, lat: p.lat };
      if (prev)
        routes.push(tripLeg(`leg:${s.id}`, prev, at, detail.legs.find((l) => l.toStopId === s.id)?.feasibility));
      bead ??= at;
      prev = at;
      allPts.push(at);
      if (s.stopKind === 'lodging') home = p;
    }
    if (bead) {
      markers.push({
        id: `bead:${day.id}`,
        position: bead,
        variant: 'bead',
        color: alarmColor(verdict),
        label: String(i + 1),
        ariaLabel:
          verdict === 'ok'
            ? tr(locale, 'plan.map.marker.day', { day: i + 1, city: day.cityHint })
            : tr(locale, 'plan.map.marker.dayAlarm', {
                day: i + 1,
                city: day.cityHint,
                alarm: tr(locale, FEASIBILITY_KEY[verdict]),
              }),
      });
    }
  }

  // city dots — centroid of each city's non-hub stops
  const cities = new Map<string, { sum: LngLat; n: number; firstDayId: string }>();
  for (const day of days) {
    for (const s of dayStopsOf(detail, day.id)) {
      const p = placeById.get(s.placeId);
      if (!p || p.kind === 'transport_hub') continue;
      const c = cities.get(day.cityHint) ?? { sum: { lng: 0, lat: 0 }, n: 0, firstDayId: day.id };
      c.sum.lng += p.lng;
      c.sum.lat += p.lat;
      c.n += 1;
      cities.set(day.cityHint, c);
    }
  }
  for (const [name, c] of cities) {
    const at = { lng: c.sum.lng / c.n, lat: c.sum.lat / c.n };
    // No colour of its own: a city dot is basemap furniture, and it takes the
    // basemap's own label ink (declared in map.css, one value per theme).
    markers.push({
      id: `city:${name}`,
      position: at,
      variant: 'city',
      tag: name,
      ariaLabel: tr(locale, 'plan.map.marker.focus', { city: name }),
    });
  }
  // multi-day cluster chips — "Days 1–3 · Tokyo", offset off the city dot
  for (let i = 0; i < days.length;) {
    let j = i;
    while (j + 1 < days.length && days[j + 1].cityHint === days[i].cityHint) j++;
    if (j > i) {
      const c = cities.get(days[i].cityHint);
      if (c) {
        markers.push({
          id: `run:${days[i].id}`,
          position: { lng: c.sum.lng / c.n - 0.32, lat: c.sum.lat / c.n + 0.16 },
          variant: 'chip',
          tag: tr(locale, 'plan.map.marker.daysInCity', { from: i + 1, to: j + 1, city: days[i].cityHint }),
        });
      }
    }
    i = j + 1;
  }
  // airports — any transport-hub stop with an IATA code in its name
  const seenAirports = new Set<string>();
  for (const s of detail.stops) {
    const p = placeById.get(s.placeId);
    const code = p?.kind === 'transport_hub' ? p.name.match(/\(([A-Z]{3})\)/)?.[1] : undefined;
    if (p && code && !seenAirports.has(code)) {
      seenAirports.add(code);
      markers.push({
        id: `apt:${code}`,
        position: { lng: p.lng, lat: p.lat },
        variant: 'transport',
        color: NEUTRAL,
        label: '✈',
        tag: code,
        interactive: false,
      });
    }
  }
  const candidateMarkers: MapMarker[] = [];
  for (const c of candidates) {
    if (c.status !== 'shortlisted') continue;
    // Every candidate gets a name and the renderer drops the ones that collide;
    // the old "is it within 0.12° of a city centroid" test guessed at a screen
    // question in geographic units and still let HND print through Ghibli
    // Museum.
    candidateMarkers.push({
      id: `cand:${c.id}`,
      position: { lng: c.place.lng, lat: c.place.lat },
      variant: 'candidate',
      tag: c.place.name,
      ariaLabel: tr(locale, 'plan.map.marker.idea', {
        place: c.place.name,
        kind: tr(locale, KIND_KEY[PLACE_KIND_STOP_KIND[c.place.kind]]),
      }),
      interactive: false,
    });
    allPts.push({ lng: c.place.lng, lat: c.place.lat });
  }
  return { routes, markers, candidateMarkers, bounds: padBounds(allPts, TRIP_PAD) };
}

/** One leg of the trip overview, in its own verdict. */
function tripLeg(id: string, from: LngLat, to: LngLat, verdict: Feasibility = 'ok'): MapRoute {
  return {
    id,
    points: [from, to],
    color: verdict === 'ok' ? 'var(--accent)' : FEASIBILITY_COLOR[verdict],
    width: verdict === 'ok' ? 3.5 : 5,
  };
}
