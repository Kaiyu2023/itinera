import type { LngLat, LngLatBounds, MapMarker, MapRoute } from '../map/MapRenderer';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';
import type { CandidateWithPlace, Day, DayFeasibility, Leg, Place, PlanDetail, Stop } from '../api/types';

/**
 * Pure map geometry — the marker/route/bounds builders shared by the desktop
 * map shell, the mobile map overlay, and the add-stop composer's embedded map.
 * Everything here is a plain function of plan data (no React, no side effects)
 * so it can be memoised at any call site and reused without duplicating logic.
 */

/** Route colours for the trip overview, one per day, cycling. */
export const DAY_COLORS = ['#4a5d8f', '#2f9e6e', '#d9a13b', '#d97b4f', '#7b5bd2', '#c4453b', '#38859b'];

export interface EdgePad {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export const DESKTOP_PAD: EdgePad = { top: 0.22, right: 0.22, bottom: 0.22, left: 0.22 };
/** Mobile: the bottom sheet covers ~45% — push the geometry into the top. */
export const SHEET_PAD: EdgePad = { top: 0.2, right: 0.24, bottom: 1.15, left: 0.24 };
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
  tagged: Set<string>; // stop ids whose name tag fits without collisions
  legs: Leg[];
}

export function buildDayGeo(detail: PlanDetail, days: Day[], day: Day, candidates: CandidateWithPlace[], pad: EdgePad): DayGeo {
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
  // Greedy tag declutter: keep a name tag only when no earlier-kept tag is
  // within ~9% of the frame — dense pairs keep the first name.
  const span = Math.max(bounds.east - bounds.west, bounds.north - bounds.south);
  const kept: Place[] = [];
  const tagged = new Set<string>();
  for (const s of stops) {
    const p = placeById.get(s.placeId);
    if (!p) continue;
    if (kept.every((k) => Math.hypot(k.lng - p.lng, k.lat - p.lat) > span * 0.09)) {
      kept.push(p);
      tagged.add(s.id);
    }
  }
  const stopIds = new Set(stops.map((s) => s.id));
  const legs = detail.legs.filter((l) => stopIds.has(l.toStopId));
  return { day, stops, placeById, feasibility: detail.dayFeasibility.find((f) => f.dayId === day.id), home, candidates: dayCands, bounds, tagged, legs };
}

/** While the add-stop composer is docked, candidate rings become live: the one
    selected in the composer wears the selected-marker treatment, and any can be
    clicked to select it there. */
export interface CandidatePick {
  interactive: boolean;
  selectedId: string | null;
}

export function dayMarkers(geo: DayGeo, selectedStopId: string | null, showCandidates: boolean, candidatePick?: CandidatePick): MapMarker[] {
  const markers: MapMarker[] = [];
  if (geo.home) {
    markers.push({
      id: 'home',
      position: { lng: geo.home.lng, lat: geo.home.lat },
      variant: 'home',
      color: KIND_COLOR.lodging,
      label: '⌂',
      tag: geo.home.name,
      tagPlacement: 'left',
      interactive: false,
    });
  }
  if (showCandidates) {
    for (const c of geo.candidates) {
      markers.push({
        id: `cand:${c.id}`,
        position: { lng: c.place.lng, lat: c.place.lat },
        variant: 'candidate',
        color: PLACE_KIND_COLOR[c.place.kind],
        tag: `${c.place.name} · candidate`,
        interactive: candidatePick?.interactive ?? false,
        selected: candidatePick?.selectedId === c.id,
      });
    }
  }
  geo.stops.forEach((s, i) => {
    const p = geo.placeById.get(s.placeId);
    if (!p) return;
    markers.push({
      id: `stop:${s.id}`,
      position: { lng: p.lng, lat: p.lat },
      variant: 'stop',
      color: KIND_COLOR[s.stopKind],
      label: String(i + 1),
      // the selected stop's popover/card replaces its name tag
      tag: geo.tagged.has(s.id) && s.id !== selectedStopId ? p.name : undefined,
      selected: s.id === selectedStopId,
    });
  });
  return markers;
}

export function dayRoutes(geo: DayGeo): MapRoute[] {
  const pts: LngLat[] = [];
  for (const s of geo.stops) {
    const p = geo.placeById.get(s.placeId);
    if (p) pts.push({ lng: p.lng, lat: p.lat });
  }
  const routes: MapRoute[] = [];
  if (geo.home && geo.stops[0] && geo.stops[0].placeId !== geo.home.id && pts[0]) {
    routes.push({
      id: 'spur',
      points: [{ lng: geo.home.lng, lat: geo.home.lat }, pts[0]],
      color: '#8a8577',
      dashed: true,
    });
  }
  routes.push({ id: 'day', points: pts, color: 'var(--color-primary)' });
  return routes;
}

/** Pins for a set of place-search hits, keyed `sr:<placeId>` so click routing
    can tell them apart from stops and candidates. */
export function searchResultMarkers(results: Place[], selectedId: string | null): MapMarker[] {
  return results.map((p) => ({
    id: `sr:${p.id}`,
    position: { lng: p.lng, lat: p.lat },
    variant: 'search-result' as const,
    color: PLACE_KIND_COLOR[p.kind],
    tag: p.name,
    selected: p.id === selectedId,
  }));
}

export interface TripGeo {
  routes: MapRoute[];
  markers: MapMarker[]; // everything except candidates (toggle-independent)
  candidateMarkers: MapMarker[];
  bounds: LngLatBounds;
}

export function buildTripGeo(detail: PlanDetail, days: Day[], candidates: CandidateWithPlace[]): TripGeo {
  const placeById = new Map(detail.places.map((p) => [p.id, p]));
  const routes: MapRoute[] = [];
  const allPts: LngLat[] = [];
  let home: Place | null = null;
  days.forEach((day, i) => {
    const stops = dayStopsOf(detail, day.id);
    const pts: LngLat[] = [];
    if (home) pts.push({ lng: home.lng, lat: home.lat });
    for (const s of stops) {
      const p = placeById.get(s.placeId);
      if (p) pts.push({ lng: p.lng, lat: p.lat });
      if (s.stopKind === 'lodging') home = placeById.get(s.placeId) ?? home;
    }
    routes.push({ id: `day:${day.id}`, points: pts, color: DAY_COLORS[i % DAY_COLORS.length] });
    allPts.push(...pts);
  });

  const markers: MapMarker[] = [];
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
    markers.push({ id: `city:${name}`, position: at, variant: 'city', color: '#55524a', tag: name });
  }
  // multi-day cluster chips — "Days 1–3 · Tokyo", offset off the city dot
  for (let i = 0; i < days.length; ) {
    let j = i;
    while (j + 1 < days.length && days[j + 1].cityHint === days[i].cityHint) j++;
    if (j > i) {
      const c = cities.get(days[i].cityHint);
      if (c) {
        markers.push({
          id: `run:${days[i].id}`,
          position: { lng: c.sum.lng / c.n - 0.32, lat: c.sum.lat / c.n + 0.16 },
          variant: 'chip',
          tag: `Days ${i + 1}–${j + 1} · ${days[i].cityHint}`,
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
        color: KIND_COLOR.transit,
        label: '✈',
        tag: code,
        interactive: false,
      });
    }
  }
  const cityPts = [...cities.values()].map((c) => ({ lng: c.sum.lng / c.n, lat: c.sum.lat / c.n }));
  const candidateMarkers: MapMarker[] = [];
  for (const c of candidates) {
    if (c.status !== 'shortlisted') continue;
    // keep the ring but drop the name when it would sit on a city label
    const nearCity = cityPts.some((p) => Math.hypot(p.lng - c.place.lng, p.lat - c.place.lat) < 0.12);
    candidateMarkers.push({
      id: `cand:${c.id}`,
      position: { lng: c.place.lng, lat: c.place.lat },
      variant: 'candidate',
      color: PLACE_KIND_COLOR[c.place.kind],
      tag: nearCity ? undefined : c.place.name,
      interactive: false,
    });
    allPts.push({ lng: c.place.lng, lat: c.place.lat });
  }
  return { routes, markers, candidateMarkers, bounds: padBounds(allPts, TRIP_PAD) };
}
