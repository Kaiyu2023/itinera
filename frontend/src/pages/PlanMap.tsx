import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, PointerEvent as ReactPointerEvent } from 'react';
import { MapView, useMapProjection } from '../map/MapView';
import type { LngLat, LngLatBounds, MapMarker, MapRoute } from '../map/MapRenderer';
import { DaylightStrip } from '../components/DaylightStrip';
import { Lightbox } from '../components/PlaceThumb';
import { formatDuration } from '../components/hooks';
import { externalMapUrl, KIND_COLOR, PLACE_KIND_COLOR, shortLegLabel } from './planShared';
import { GovModalHost, PlanActionsProvider, ProposeStopComposer, usePlanActions, usePlanActionsState } from './PlanGovernance';
import type {
  CandidateWithPlace,
  Day,
  DayFeasibility,
  Leg,
  Place,
  PlanDetail,
  Stop,
  StopKind,
  Thread,
  User,
} from '../api/types';

/** Route colours for the trip overview, one per day, cycling. */
export const DAY_COLORS = ['#4a5d8f', '#2f9e6e', '#d9a13b', '#d97b4f', '#7b5bd2', '#c4453b', '#38859b'];

/** Panel/scrubber selection: a day id, or the whole-trip overview. */
export type MapSelection = string; // dayId | 'trip'

/* ═══════════════ geometry builders (pure, memo-friendly) ═══════════════ */

interface EdgePad {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

const DESKTOP_PAD: EdgePad = { top: 0.22, right: 0.22, bottom: 0.22, left: 0.22 };
/** Mobile: the bottom sheet covers ~45% — push the geometry into the top. */
const SHEET_PAD: EdgePad = { top: 0.2, right: 0.24, bottom: 1.15, left: 0.24 };
const TRIP_PAD: EdgePad = { top: 0.14, right: 0.14, bottom: 0.14, left: 0.14 };

function padBounds(points: LngLat[], pad: EdgePad): LngLatBounds {
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

function inBounds(p: { lng: number; lat: number }, b: LngLatBounds): boolean {
  return p.lng >= b.west && p.lng <= b.east && p.lat >= b.south && p.lat <= b.north;
}

function dayStopsOf(detail: PlanDetail, dayId: string): Stop[] {
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

interface DayGeo {
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

function buildDayGeo(detail: PlanDetail, days: Day[], day: Day, candidates: CandidateWithPlace[], pad: EdgePad): DayGeo {
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
interface CandidatePick {
  interactive: boolean;
  selectedId: string | null;
}

function dayMarkers(geo: DayGeo, selectedStopId: string | null, showCandidates: boolean, candidatePick?: CandidatePick): MapMarker[] {
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

function dayRoutes(geo: DayGeo): MapRoute[] {
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

interface TripGeo {
  routes: MapRoute[];
  markers: MapMarker[]; // everything except candidates (toggle-independent)
  candidateMarkers: MapMarker[];
  bounds: LngLatBounds;
}

function buildTripGeo(detail: PlanDetail, days: Day[], candidates: CandidateWithPlace[]): TripGeo {
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

/* ═══════════════ shared bits ═══════════════ */

function fmtDay(date: string, opts: Intl.DateTimeFormatOptions): string {
  return new Date(date + 'T00:00:00').toLocaleDateString(undefined, opts);
}

function MapScrubber({ days, active, onSelect }: { days: Day[]; active: MapSelection; onSelect: (v: MapSelection) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current
      ?.querySelector('[aria-selected="true"]')
      ?.scrollIntoView({ inline: 'center', block: 'nearest' });
  }, [active]);
  return (
    <div ref={ref} className="map-scrub" role="tablist" aria-label="Map focus">
      <button
        role="tab"
        aria-selected={active === 'trip'}
        className={`day-chip${active === 'trip' ? ' active' : ''}`}
        onClick={() => onSelect('trip')}
      >
        🗾 Trip
      </button>
      {days.map((day) => (
        <button
          key={day.id}
          role="tab"
          aria-selected={active === day.id}
          className={`day-chip${active === day.id ? ' active' : ''}`}
          onClick={() => onSelect(day.id)}
        >
          {fmtDay(day.date, { weekday: 'short', day: 'numeric' })}
        </button>
      ))}
    </div>
  );
}

function MapDayHead({ geo, dayIndex, compact }: { geo: DayGeo; dayIndex: number; compact?: boolean }) {
  const f = geo.feasibility;
  const longDate = fmtDay(geo.day.date, { weekday: 'short', month: 'short', day: 'numeric' });
  return (
    <div className="day-head map-day-head">
      <div className="day-numblock">
        <span className="day-eyebrow">Day</span>
        <span className="day-num">{String(dayIndex + 1).padStart(2, '0')}</span>
      </div>
      <div>
        <h2 className="day-city">
          {geo.day.cityHint}
          {f && (
            <span className={`badge ${f.feasibility} verdict-inline`}>
              {f.feasibility} · {Math.round((f.usedMin / f.windowMin) * 100)}%
            </span>
          )}
        </h2>
        <p className="map-day-meta">
          {compact
            ? `${longDate} · ${geo.day.windowStart}–${geo.day.windowEnd}${f ? ` · ${f.usedMin} / ${f.windowMin} min` : ''}`
            : `${longDate} · window ${geo.day.windowStart}–${geo.day.windowEnd}${geo.home ? ` · ${geo.home.name}` : ''}`}
        </p>
      </div>
    </div>
  );
}

function CompactStopList({
  geo,
  kindLabels,
  selectedId,
  onSelect,
}: {
  geo: DayGeo;
  kindLabels: Record<StopKind, string>;
  selectedId: string | null;
  onSelect: (stopId: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!selectedId) return;
    listRef.current
      ?.querySelector(`[data-stop="${selectedId}"]`)
      ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedId]);

  return (
    <div ref={listRef}>
      {geo.stops.map((stop, i) => {
        const place = geo.placeById.get(stop.placeId);
        const legIn = geo.legs.find((l) => l.toStopId === stop.id);
        const next = geo.stops[i + 1];
        const legOut = next ? geo.legs.find((l) => l.toStopId === next.id) : undefined;
        return (
          <div key={stop.id} className="ms-row" style={{ '--kc': KIND_COLOR[stop.stopKind] } as CSSProperties}>
            <span className="ms-time">{stop.plannedArrival}</span>
            <span className="ms-rail">
              <span className="ms-dot" />
            </span>
            <div>
              {i === 0 && legIn && (
                <div className="ms-leg first">
                  <span className="leg-chip">{shortLegLabel(legIn)}</span>
                </div>
              )}
              <button
                type="button"
                data-stop={stop.id}
                className={`ms-card${stop.id === selectedId ? ' sel' : ''}`}
                onClick={() => onSelect(stop.id)}
              >
                <span className="ms-name">
                  {place?.name ?? stop.placeId}
                  <span className="ms-kind" style={{ color: KIND_COLOR[stop.stopKind] }}>
                    {kindLabels[stop.stopKind]}
                  </span>
                  {stop.booking && <span className="badge">booked</span>}
                </span>
                <span className="ms-meta">
                  {stop.plannedArrival} · {formatDuration(stop.durationMin)}
                </span>
              </button>
              {legOut && (
                <div className="ms-leg">
                  <span className={`leg-chip${legOut.feasibility !== 'ok' ? ` ${legOut.feasibility}` : ''}`}>
                    {shortLegLabel(legOut)}
                  </span>
                </div>
              )}
            </div>
          </div>
        );
      })}
      <ProposeStopButton day={geo.day} />
    </div>
  );
}

function TripPanel({
  detail,
  days,
  candidates,
  membersById,
  onSelectDay,
}: {
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  membersById: Map<string, User>;
  onSelectDay: (dayId: string) => void;
}) {
  const shortlisted = candidates.filter((c) => c.status === 'shortlisted');
  return (
    <>
      <div className="panel-h">
        The route — {days.length} days · {detail.stops.length} stops
      </div>
      {days.map((day, i) => {
        const f = detail.dayFeasibility.find((x) => x.dayId === day.id);
        const n = detail.stops.filter((s) => s.dayId === day.id).length;
        return (
          <button type="button" key={day.id} className="trow" onClick={() => onSelectDay(day.id)}>
            <span className="sw" style={{ background: DAY_COLORS[i % DAY_COLORS.length] }} />
            <span className="trow-main">
              <span className="nm">
                Day {String(i + 1).padStart(2, '0')} · {day.cityHint}
              </span>
              <span className="sub">
                {fmtDay(day.date, { weekday: 'short', day: 'numeric' })} · {n} stops
              </span>
            </span>
            {f && <span className="vd" style={{ background: `var(--color-${f.feasibility})` }} title={f.feasibility} />}
          </button>
        );
      })}
      <div className="panel-h">Candidates still in play — {shortlisted.length}</div>
      {shortlisted.map((c) => (
        <div key={c.id} className="crow" style={{ '--kc': PLACE_KIND_COLOR[c.place.kind] } as CSSProperties}>
          <span className="ring" />
          <span className="trow-main">
            <span className="nm">{c.place.name}</span>
            <span className="sub">
              {c.place.city}
              {c.tags[0] ? ` · ${c.tags[0]}` : ''}
            </span>
          </span>
          <span className="by">{membersById.get(c.proposedBy)?.displayName}</span>
        </div>
      ))}
    </>
  );
}

function CandidatesLayerToggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button type="button" className={`ctl-candidates${on ? ' on' : ''}`} onClick={onToggle} aria-pressed={on}>
      <span>◌ Candidates</span>
      <span className="sw" />
    </button>
  );
}

/** Banner photo on the popover / sheet card — click opens the shared lightbox. */
function PhotoBanner({ place }: { place: Place }) {
  const [viewer, setViewer] = useState<number | null>(null);
  const photos = place.photoUrls;
  if (photos.length === 0) return null;
  return (
    <>
      <button
        type="button"
        className="photo-banner"
        onClick={() => setViewer(0)}
        aria-label={photos.length > 1 ? `View ${photos.length} photos of ${place.name}` : `View photo of ${place.name}`}
      >
        <img src={photos[0]} alt="" />
        {photos.length > 1 && (
          <span className="thumb-more" aria-hidden="true">
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4">
              <rect x="3.5" y="3.5" width="7" height="7" rx="1.5" />
              <path d="M8.5 1.5h-6a1.5 1.5 0 0 0-1.5 1.5v6" />
            </svg>
            {photos.length}
          </span>
        )}
      </button>
      {viewer != null && (
        <Lightbox photos={photos} name={place.name} index={viewer} onIndex={setViewer} onClose={() => setViewer(null)} />
      )}
    </>
  );
}

function ProposeStopButton({ day }: { day: Day }) {
  const actions = usePlanActions();
  return (
    <button type="button" className="ghost-btn" onClick={() => actions.proposeStop(day)}>
      ＋ Propose a stop on this day
    </button>
  );
}

function StopActions({ stop, place, threads }: { stop: Stop; place: Place; threads: Thread[] }) {
  const thread = threads.find((t) => t.anchor.kind === 'stop' && t.anchor.stopId === stop.id);
  const actions = usePlanActions();
  return (
    <div className="stop-actions">
      <button type="button" className="b" onClick={() => actions.discuss(stop)}>
        💬 Discuss{thread ? ` · ${thread.commentCount}` : ''}
      </button>
      <button type="button" className="b" onClick={() => actions.proposeChange(stop)}>
        ✎ Propose change
      </button>
      <a className="b link" href={externalMapUrl(place)} target="_blank" rel="noreferrer">
        Maps ↗
      </a>
    </div>
  );
}

/** Rich stop card floating on the map, anchored to its marker via project(). */
function StopPopover({
  geo,
  stop,
  kindLabels,
  candidates,
  threads,
}: {
  geo: DayGeo;
  stop: Stop;
  kindLabels: Record<StopKind, string>;
  candidates: CandidateWithPlace[];
  threads: Thread[];
}) {
  const projection = useMapProjection();
  const ref = useRef<HTMLDivElement>(null);
  const [layout, setLayout] = useState<{ left: number; top: number; side: 'left' | 'right'; arrowTop: number } | null>(null);
  const place = geo.placeById.get(stop.placeId);

  useLayoutEffect(() => {
    const el = ref.current;
    const frame = el?.parentElement;
    if (!el || !frame || !place || !projection) return;
    const pos = projection.project({ lng: place.lng, lat: place.lat });
    if (!pos) return;
    const bw = el.offsetWidth;
    const bh = el.offsetHeight;
    const fw = frame.clientWidth;
    const fh = frame.clientHeight;
    const side: 'left' | 'right' = pos.x - bw - 26 >= 6 ? 'left' : 'right';
    const left = Math.max(8, Math.min(side === 'left' ? pos.x - bw - 20 : pos.x + 20, fw - bw - 8));
    const top = Math.min(Math.max(pos.y - bh + 48, 8), Math.max(8, fh - bh - 8));
    const arrowTop = Math.min(Math.max(pos.y - top - 7, 14), bh - 26);
    setLayout({ left, top, side, arrowTop });
  }, [stop.id, place, projection]);

  if (!place) return null;
  const cand = candidates.find((c) => c.placeId === stop.placeId);
  return (
    <div
      ref={ref}
      className={`map-popover side-${layout?.side ?? 'left'}${layout ? '' : ' measuring'}`}
      style={layout ? { left: layout.left, top: layout.top } : undefined}
    >
      <PhotoBanner place={place} />
      <div className="pc">
        <div className="row1">
          <span style={{ color: KIND_COLOR[stop.stopKind] }}>{kindLabels[stop.stopKind]}</span>
          {cand && cand.tags.length > 0 && <span className="tags">{cand.tags.join(' · ')}</span>}
          {place.rating != null && <span className="rating">★ {place.rating.toFixed(1)}</span>}
        </div>
        <h3>{place.name}</h3>
        {place.openingHours && <div className="hrs">{place.openingHours.weekdayText[0]}</div>}
        <div className="when">
          arrive {stop.plannedArrival} · stay {formatDuration(stop.durationMin)}
        </div>
        {stop.notes && <div className="note">{stop.notes}</div>}
        <StopActions stop={stop} place={place} threads={threads} />
      </div>
      <span className="arrow" style={layout ? { top: layout.arrowTop } : undefined} />
    </div>
  );
}

/* ═══════════════ desktop: panel + map card ═══════════════ */

export interface PlanMapProps {
  tripId: string;
  detail: PlanDetail;
  days: Day[];
  kindLabels: Record<StopKind, string>;
  candidates: CandidateWithPlace[];
  membersById: Map<string, User>;
  threads: Thread[];
  active: MapSelection;
  onSelect: (v: MapSelection) => void;
  initialStopId?: string | null;
}

export function PlanMapShell({ tripId, detail, days, kindLabels, candidates, membersById, threads, active, onSelect, initialStopId }: PlanMapProps) {
  const [selectedStopId, setSelectedStopId] = useState<string | null>(initialStopId ?? null);
  const [showCandidates, setShowCandidates] = useState(true);
  const gov = usePlanActionsState();

  // The add-stop composer docks into this panel instead of a modal. Its
  // candidate selection lives here so it can drive the map markers (and be
  // driven by clicking them). It targets the day the "+" button belonged to.
  const dockedDay = gov.action?.kind === 'addStop' ? gov.action.day : null;
  const [addMode, setAddMode] = useState<'candidates' | 'new'>('candidates');
  const [addCandidateId, setAddCandidateId] = useState('');

  const prevActive = useRef(active);
  useEffect(() => {
    if (prevActive.current !== active) {
      prevActive.current = active;
      setSelectedStopId(null);
    }
  }, [active]);

  // One Escape handler, topmost-surface-wins: a photo lightbox and the gov
  // surfaces (modal, or the docked composer) each own Escape while up; only
  // then does it dismiss the stop popover.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (document.querySelector('.lb-backdrop')) return; // lightbox
      if (gov.action) { gov.close(); return; } // modal or docked composer
      if (selectedStopId) setSelectedStopId(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [selectedStopId, gov]);

  const activeDay = active === 'trip' ? null : days.find((d) => d.id === active) ?? days[0];
  const dayGeo = useMemo(
    () => (activeDay ? buildDayGeo(detail, days, activeDay, candidates, DESKTOP_PAD) : null),
    [detail, days, activeDay, candidates],
  );
  const tripGeo = useMemo(
    () => (activeDay ? null : buildTripGeo(detail, days, candidates)),
    [detail, days, candidates, activeDay],
  );

  // On opening the docked composer, default to a candidate that's actually on
  // this day's map (so its ring highlights) — else the first shortlisted.
  useEffect(() => {
    if (!dockedDay) return;
    setAddMode('candidates');
    setAddCandidateId(dayGeo?.candidates[0]?.id ?? candidates.find((c) => c.status === 'shortlisted')?.id ?? '');
  }, [dockedDay?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  // The composer only lights up markers for the day it's editing.
  const composerHere = !!dockedDay && !!activeDay && dockedDay.id === activeDay.id;
  const candidatePick = composerHere && addMode === 'candidates' ? { interactive: true, selectedId: addCandidateId } : undefined;

  const markers = useMemo(() => {
    if (dayGeo) return dayMarkers(dayGeo, selectedStopId, showCandidates, candidatePick);
    if (tripGeo) return showCandidates ? [...tripGeo.markers, ...tripGeo.candidateMarkers] : tripGeo.markers;
    return [];
  }, [dayGeo, tripGeo, selectedStopId, showCandidates, candidatePick]);
  const routes = useMemo(() => (dayGeo ? dayRoutes(dayGeo) : tripGeo ? tripGeo.routes : []), [dayGeo, tripGeo]);
  const bounds = dayGeo ? dayGeo.bounds : tripGeo!.bounds;

  const selectedStop = dayGeo?.stops.find((s) => s.id === selectedStopId) ?? null;

  const handleMarkerClick = (id: string) => {
    if (id.startsWith('cand:') && composerHere) {
      // Clicking a candidate ring selects it in the docked composer.
      setAddMode('candidates');
      setAddCandidateId(id.slice(5));
    } else if (id.startsWith('stop:')) setSelectedStopId(id.slice(5));
    else if (id.startsWith('city:')) {
      const day = days.find((d) => d.cityHint === id.slice(5));
      if (day) onSelect(day.id);
    } else if (id.startsWith('run:')) onSelect(id.slice(4));
  };

  return (
    <PlanActionsProvider actions={gov.actions}>
    <div className="map-shell">
      <aside className="map-panel">
        <div className="map-panel-body">
          {dockedDay ? (
            <ProposeStopComposer
              day={dockedDay}
              detail={detail}
              candidates={candidates}
              tripId={tripId}
              onClose={gov.close}
              docked
              mode={addMode}
              onModeChange={setAddMode}
              candidateId={addCandidateId}
              onCandidateChange={setAddCandidateId}
            />
          ) : (
            <>
              <MapScrubber days={days} active={active} onSelect={onSelect} />
              {dayGeo && activeDay ? (
                <>
                  <MapDayHead geo={dayGeo} dayIndex={days.indexOf(activeDay)} />
                  {dayGeo.feasibility && dayGeo.feasibility.notes.length > 0 && (
                    <ul className="map-notes">
                      {dayGeo.feasibility.notes.map((note) => (
                        <li key={note}>{note}</li>
                      ))}
                    </ul>
                  )}
                  <DaylightStrip day={activeDay} detail={detail} stops={dayGeo.stops} />
                  <CompactStopList geo={dayGeo} kindLabels={kindLabels} selectedId={selectedStopId} onSelect={setSelectedStopId} />
                </>
              ) : (
                <TripPanel detail={detail} days={days} candidates={candidates} membersById={membersById} onSelectDay={onSelect} />
              )}
            </>
          )}
        </div>
      </aside>
      <MapView
        markers={markers}
        routes={routes}
        bounds={bounds}
        onMarkerClick={handleMarkerClick}
        onMapClick={() => setSelectedStopId(null)}
      >
        <CandidatesLayerToggle on={showCandidates} onToggle={() => setShowCandidates((v) => !v)} />
        {dayGeo && selectedStop && (
          <StopPopover geo={dayGeo} stop={selectedStop} kindLabels={kindLabels} candidates={candidates} threads={threads} />
        )}
      </MapView>
    </div>
    {/* Discuss + Propose-change stay modal; add-stop is docked above. */}
    <GovModalHost action={gov.action} close={gov.close} dockAddStop tripId={tripId} detail={detail} days={days} candidates={candidates} membersById={membersById} threads={threads} />
    </PlanActionsProvider>
  );
}

/* ═══════════════ mobile: full-screen map + bottom sheet ═══════════════ */

export function PlanMapOverlay({
  onClose,
  tripId,
  detail,
  days,
  kindLabels,
  candidates,
  membersById,
  threads,
  active,
  onSelect,
  initialStopId,
}: PlanMapProps & { onClose: () => void }) {
  const activeDay = active === 'trip' ? null : days.find((d) => d.id === active) ?? days[0];
  const dayGeo = useMemo(
    () => (activeDay ? buildDayGeo(detail, days, activeDay, candidates, SHEET_PAD) : null),
    [detail, days, activeDay, candidates],
  );
  const tripGeo = useMemo(
    () => (activeDay ? null : buildTripGeo(detail, days, candidates)),
    [detail, days, candidates, activeDay],
  );

  const [selectedStopId, setSelectedStopId] = useState<string | null>(initialStopId ?? null);
  const [showCandidates, setShowCandidates] = useState(true);
  const [expanded, setExpanded] = useState(true);
  const dragStart = useRef<number | null>(null);
  const gov = usePlanActionsState();

  // Featured stop defaults to the day's first; day change re-anchors it.
  const prevActive = useRef(active);
  useEffect(() => {
    if (prevActive.current !== active) {
      prevActive.current = active;
      setSelectedStopId(null);
    }
  }, [active]);
  const selectedStop = dayGeo ? dayGeo.stops.find((s) => s.id === selectedStopId) ?? dayGeo.stops[0] ?? null : null;

  useEffect(() => {
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = '';
    };
  }, []);

  const markers = useMemo(() => {
    if (dayGeo) return dayMarkers(dayGeo, selectedStop?.id ?? null, showCandidates);
    if (tripGeo) return showCandidates ? [...tripGeo.markers, ...tripGeo.candidateMarkers] : tripGeo.markers;
    return [];
  }, [dayGeo, tripGeo, selectedStop, showCandidates]);
  const routes = useMemo(() => (dayGeo ? dayRoutes(dayGeo) : tripGeo ? tripGeo.routes : []), [dayGeo, tripGeo]);
  const bounds = dayGeo ? dayGeo.bounds : tripGeo!.bounds;

  const onGripDown = (e: ReactPointerEvent) => {
    dragStart.current = e.clientY;
  };
  const onGripUp = (e: ReactPointerEvent) => {
    const start = dragStart.current;
    dragStart.current = null;
    if (start == null) return;
    const delta = e.clientY - start;
    if (Math.abs(delta) < 8) setExpanded((v) => !v);
    else setExpanded(delta < 0);
  };

  // The sheet lists the whole day in order; the selected stop becomes the rich
  // card in place, and selecting keeps it in view.
  const sheetBodyRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!selectedStop) return;
    sheetBodyRef.current
      ?.querySelector(`[data-stop="${selectedStop.id}"]`)
      ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedStop?.id]);

  return (
    <PlanActionsProvider actions={gov.actions}>
    <div className="map-overlay">
      <MapView
        markers={markers}
        routes={routes}
        bounds={bounds}
        padding={16}
        className="map-overlay-map"
        onMarkerClick={(id) => {
          if (id.startsWith('stop:')) {
            setSelectedStopId(id.slice(5));
            setExpanded(true);
          } else if (id.startsWith('city:')) {
            const day = days.find((d) => d.cityHint === id.slice(5));
            if (day) onSelect(day.id);
          } else if (id.startsWith('run:')) onSelect(id.slice(4));
        }}
      >
        <CandidatesLayerToggle on={showCandidates} onToggle={() => setShowCandidates((v) => !v)} />
      </MapView>

      <div className="m-top">
        <button type="button" className="m-back" onClick={onClose}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" aria-hidden>
            <path d="M4 6h16" /> <path d="M4 12h16" /> <path d="M4 18h10" />
          </svg>
          Timeline
        </button>
        <MapScrubber days={days} active={active} onSelect={onSelect} />
      </div>

      <div className={`map-sheet${expanded ? '' : ' collapsed'}`}>
        <div className="sheet-grip" onPointerDown={onGripDown} onPointerUp={onGripUp}>
          <span className="handle" />
        </div>
        {dayGeo && activeDay ? (
          <>
            <MapDayHead geo={dayGeo} dayIndex={days.indexOf(activeDay)} compact />
            <div className="sheet-body" ref={sheetBodyRef}>
              {dayGeo.stops.map((stop, i) => {
                const place = dayGeo.placeById.get(stop.placeId);
                const legIn = dayGeo.legs.find((l) => l.toStopId === stop.id);
                const featured = stop.id === selectedStop?.id;
                return (
                  <Fragment key={stop.id}>
                    {legIn && (
                      <div className={`ms-leg${i === 0 ? ' first' : ''}`}>
                        <span className={`leg-chip${legIn.feasibility !== 'ok' ? ` ${legIn.feasibility}` : ''}`}>
                          {shortLegLabel(legIn)}
                        </span>
                      </div>
                    )}
                    {featured && place ? (
                      <div className="m-card" data-stop={stop.id}>
                        <PhotoBanner place={place} />
                        <div className="body">
                          <div className="m-card-head">
                            <strong>{place.name}</strong>
                            <span className="ms-kind" style={{ color: KIND_COLOR[stop.stopKind] }}>
                              {kindLabels[stop.stopKind]}
                            </span>
                            {place.rating != null && <span className="m-rating">★ {place.rating.toFixed(1)}</span>}
                          </div>
                          <div className="m-card-meta">
                            arrive {stop.plannedArrival} · stay {formatDuration(stop.durationMin)}
                            {place.openingHours ? ` · ${place.openingHours.weekdayText[0]}` : ''}
                          </div>
                          <StopActions stop={stop} place={place} threads={threads} />
                        </div>
                      </div>
                    ) : (
                      <button
                        type="button"
                        data-stop={stop.id}
                        className="ms-card"
                        onClick={() => setSelectedStopId(stop.id)}
                      >
                        <span className="ms-name">
                          {place?.name ?? stop.placeId}
                          <span className="ms-kind" style={{ color: KIND_COLOR[stop.stopKind] }}>
                            {kindLabels[stop.stopKind]}
                          </span>
                        </span>
                        <span className="ms-meta">
                          {stop.plannedArrival} · {formatDuration(stop.durationMin)}
                        </span>
                      </button>
                    )}
                  </Fragment>
                );
              })}
              <ProposeStopButton day={dayGeo.day} />
            </div>
          </>
        ) : (
          <div className="sheet-body">
            <TripPanel detail={detail} days={days} candidates={candidates} membersById={membersById} onSelectDay={onSelect} />
          </div>
        )}
      </div>
    </div>
    {/* All three surfaces rise as bottom sheets on mobile. */}
    <GovModalHost action={gov.action} close={gov.close} tripId={tripId} detail={detail} days={days} candidates={candidates} membersById={membersById} threads={threads} />
    </PlanActionsProvider>
  );
}

/** Floating "open the map" pill — the mobile entry point to the map sheet. */
export function MapPill({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="map-pill" onClick={onClick}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <path d="M5 17.5c0-4.5 4.5-3.5 5.5-7.5S16 6 16 6" strokeDasharray="0.1 3.4" />
        <circle cx="5" cy="18.5" r="2.2" fill="currentColor" stroke="none" />
        <circle cx="17.5" cy="5.5" r="2.2" fill="currentColor" stroke="none" />
      </svg>
      Map
    </button>
  );
}
