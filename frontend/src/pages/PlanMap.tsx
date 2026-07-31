import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, PointerEvent as ReactPointerEvent } from 'react';
import { useSearchParams } from 'react-router';
import { MapView, useMapProjection } from '../map/MapView';
import { DaylightStrip } from '../components/DaylightStrip';
import { KindGlyph } from '../components/KindGlyph';
import { Lightbox } from '../components/PlaceThumb';
import { formatDuration } from '../components/hooks';
import { useModalChrome } from '../components/useModalChrome';
import { externalMapUrl, PLACE_KIND_STOP_KIND, shortLegLabel } from './planShared';
import {
  DESKTOP_PAD,
  SHEET_PAD,
  buildDayGeo,
  buildTripGeo,
  dayMarkers,
  dayRoutes,
  padBounds,
  proposedDayRoutes,
  proposedStopMarker,
  searchResultMarkers,
  stopAlarm,
} from './planMapGeometry';
import type { DayGeo } from './planMapGeometry';
import type { EdgePadPx, LngLat } from '../map/MapRenderer';
import {
  ProposeStopComposer,
  readAddStopDeepLink,
  stripAddStopDeepLink,
  usePlanActions,
  useStopSearch,
} from './PlanGovernance';
import type { GovState } from './PlanGovernance';
import type { CandidateWithPlace, Day, Place, PlanDetail, Stop, StopKind, Thread, User } from '../api/types';

/** Panel/scrubber selection: a day id, or the whole-trip overview. */
export type MapSelection = string; // dayId | 'trip'

/* ═══════════════ shared bits ═══════════════ */

function fmtDay(date: string, opts: Intl.DateTimeFormatOptions): string {
  return new Date(date + 'T00:00:00').toLocaleDateString(undefined, opts);
}

function MapScrubber({
  days,
  active,
  onSelect,
}: {
  days: Day[];
  active: MapSelection;
  onSelect: (v: MapSelection) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector('[aria-pressed="true"]')?.scrollIntoView({ inline: 'center', block: 'nearest' });
  }, [active]);
  // Not a tablist: these chips control a canvas, not a set of tabpanels, so
  // there was nothing for `aria-controls` to point at and no tabpanel for a
  // screen reader to move into. They are what they look like — a group of
  // toggle buttons, exactly one of them pressed.
  return (
    <div ref={ref} className="map-scrub" role="group" aria-label="Map focus">
      <button
        type="button"
        aria-pressed={active === 'trip'}
        className={`day-chip${active === 'trip' ? ' active' : ''}`}
        onClick={() => onSelect('trip')}
      >
        🗾 Trip
      </button>
      {days.map((day) => (
        <button
          key={day.id}
          type="button"
          aria-pressed={active === day.id}
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
        // The rail dot answers the same question the pin does: is anything
        // wrong here? It used to be the stop's kind hue, which put a green dot
        // for "activity" beside a green chip for "settled".
        const alarm = stopAlarm(stop, next, geo.legs);
        return (
          <div
            key={stop.id}
            className={`ms-row${alarm === 'ok' ? '' : ' alarm'}`}
            style={alarm === 'ok' ? undefined : ({ '--kc': `var(--color-${alarm})` } as CSSProperties)}
          >
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
                  <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
                  {place?.name ?? stop.placeId}
                  <span className="ms-kind">{kindLabels[stop.stopKind]}</span>
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
      {/* The swatch that used to sit here was an 8px day colour keyed to a
          per-day route line on the map — a line that no longer exists, because
          seven of them ran the same corridor and only the last was visible.
          With it gone the row has one coloured thing on it, and that thing is
          the verdict. The verdict is also a word now: it was an 8px dot with
          the reading hidden in a `title` tooltip, i.e. colour-only, and green
          "day 2" sat next to green "ok" while amber "day 3" sat next to amber
          "tight". */}
      {days.map((day, i) => {
        const f = detail.dayFeasibility.find((x) => x.dayId === day.id);
        const n = detail.stops.filter((s) => s.dayId === day.id).length;
        return (
          <button type="button" key={day.id} className="trow" onClick={() => onSelectDay(day.id)}>
            <span className="daynum">{String(i + 1).padStart(2, '0')}</span>
            <span className="trow-main">
              <span className="nm">{day.cityHint}</span>
              <span className="sub">
                {fmtDay(day.date, { weekday: 'short', day: 'numeric' })} · {n} stops
              </span>
            </span>
            {f && f.feasibility !== 'ok' && <span className={`badge ${f.feasibility}`}>{f.feasibility}</span>}
          </button>
        );
      })}
      <div className="panel-h">Candidates still in play — {shortlisted.length}</div>
      {shortlisted.map((c) => (
        <div key={c.id} className="crow">
          <span className="ring" />
          <span className="trow-main">
            <span className="nm">
              <KindGlyph kind={PLACE_KIND_STOP_KIND[c.place.kind]} />
              {c.place.name}
            </span>
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
    <button
      type="button"
      className={`ctl-candidates${on ? ' on' : ''}`}
      onClick={onToggle}
      aria-pressed={on}
      // Permanently covers a corner of the frame, so the renderer's tag
      // declutter has to treat it as occupied (see MapRenderer).
      data-map-chrome
    >
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
        <Lightbox
          photos={photos}
          name={place.name}
          index={viewer}
          onIndex={setViewer}
          onClose={() => setViewer(null)}
        />
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
  const [layout, setLayout] = useState<{ left: number; top: number; side: 'left' | 'right'; arrowTop: number } | null>(
    null,
  );
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
          <span className="pc-kind">
            <KindGlyph kind={stop.stopKind} />
            {kindLabels[stop.stopKind]}
          </span>
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
  /** Governance state is hoisted to PlanTab so only one host owns the modals. */
  gov: GovState;
}

export function PlanMapShell({
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
  gov,
}: PlanMapProps) {
  const [selectedStopId, setSelectedStopId] = useState<string | null>(initialStopId ?? null);
  const [showCandidates, setShowCandidates] = useState(true);

  // The add-stop composer docks into this panel instead of a modal. Its
  // candidate selection lives here so it can drive the map markers (and be
  // driven by clicking them). It targets the day the "+" button belonged to.
  const dockedDay = gov.action?.kind === 'addStop' ? gov.action.day : null;
  const [addMode, setAddMode] = useState<'candidates' | 'new'>('candidates');
  const [addCandidateId, setAddCandidateId] = useState('');
  // The composer reports its insert-outcome preview (point + new seq) up here so
  // it can be spliced onto the live day map.
  const [addPreview, setAddPreview] = useState<{ insertAt: LngLat; seq: number } | null>(null);
  // Place-search state lives here too so its hits become temporary pins on the
  // live map (two-way selectable), just like the candidate rings.
  const search = useStopSearch();
  const searchRef = useRef(search);
  searchRef.current = search;
  const [urlParams, setUrlParams] = useSearchParams();
  const urlRef = useRef({ params: urlParams, set: setUrlParams });
  urlRef.current = { params: urlParams, set: setUrlParams };

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
      if (gov.action) {
        gov.close();
        return;
      } // modal or docked composer
      if (selectedStopId) setSelectedStopId(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [selectedStopId, gov]);

  const activeDay = active === 'trip' ? null : (days.find((d) => d.id === active) ?? days[0]);
  const dayGeo = useMemo(
    () => (activeDay ? buildDayGeo(detail, days, activeDay, candidates, DESKTOP_PAD) : null),
    [detail, days, activeDay, candidates],
  );
  const tripGeo = useMemo(
    () => (activeDay ? null : buildTripGeo(detail, days, candidates)),
    [detail, days, candidates, activeDay],
  );

  // On opening the docked composer, default to a candidate that's actually on
  // this day's map (so its ring highlights) — else the first shortlisted. The
  // search box starts empty each time the composer opens.
  useEffect(() => {
    if (!dockedDay) return;
    setAddCandidateId(dayGeo?.candidates[0]?.id ?? candidates.find((c) => c.status === 'shortlisted')?.id ?? '');
    searchRef.current.clear();
    // A one-shot ?gov=addStop deep link can open the composer straight into
    // "somewhere new" with a query primed; strip it so later opens are clean.
    const link = readAddStopDeepLink(urlRef.current.params, dockedDay.id);
    if (link) {
      setAddMode(link.mode ?? 'candidates');
      if (link.query) {
        searchRef.current.setQuery(link.query);
        if (link.pickFirst) searchRef.current.pickFirstOnNext();
      }
      urlRef.current.set(stripAddStopDeepLink(urlRef.current.params), { replace: true });
    } else {
      setAddMode('candidates');
    }
  }, [dockedDay?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  // The composer only lights up markers for the day it's editing.
  const composerHere = !!dockedDay && !!activeDay && dockedDay.id === activeDay.id;
  const candidatePick =
    composerHere && addMode === 'candidates' ? { interactive: true, selectedId: addCandidateId } : undefined;
  const showSearchPins = composerHere && addMode === 'new' && search.results.length > 0;
  // Only splice the outcome preview onto the map while the composer edits this day.
  const previewHere = composerHere ? addPreview : null;

  const markers = useMemo(() => {
    if (dayGeo) {
      const base = dayMarkers(dayGeo, selectedStopId, showCandidates, candidatePick);
      const withHits = showSearchPins ? [...base, ...searchResultMarkers(search.results, search.selectedId)] : base;
      return previewHere ? [...withHits, proposedStopMarker(previewHere.insertAt, previewHere.seq)] : withHits;
    }
    if (tripGeo) return showCandidates ? [...tripGeo.markers, ...tripGeo.candidateMarkers] : tripGeo.markers;
    return [];
  }, [
    dayGeo,
    tripGeo,
    selectedStopId,
    showCandidates,
    candidatePick,
    showSearchPins,
    search.results,
    search.selectedId,
    previewHere,
  ]);
  const routes = useMemo(() => {
    if (dayGeo)
      return previewHere ? proposedDayRoutes(dayGeo, previewHere.insertAt, previewHere.seq) : dayRoutes(dayGeo);
    return tripGeo ? tripGeo.routes : [];
  }, [dayGeo, tripGeo, previewHere]);
  // When search pins or an outcome preview are live, widen the frame so out-of-day
  // hits / the inserted point still show.
  const bounds = useMemo(() => {
    if (dayGeo && (showSearchPins || previewHere)) {
      const dayPts = dayGeo.stops
        .map((s) => dayGeo.placeById.get(s.placeId))
        .filter((p): p is Place => !!p)
        .map((p) => ({ lng: p.lng, lat: p.lat }));
      const extra: LngLat[] = [];
      if (showSearchPins) extra.push(...search.results.map((r) => ({ lng: r.lng, lat: r.lat })));
      if (previewHere) extra.push(previewHere.insertAt);
      return padBounds([...dayPts, ...extra], DESKTOP_PAD);
    }
    return dayGeo ? dayGeo.bounds : tripGeo!.bounds;
  }, [dayGeo, tripGeo, showSearchPins, search.results, previewHere]);

  const selectedStop = dayGeo?.stops.find((s) => s.id === selectedStopId) ?? null;

  const handleMarkerClick = (id: string) => {
    if (id.startsWith('cand:') && composerHere) {
      // Clicking a candidate ring selects it in the docked composer.
      setAddMode('candidates');
      setAddCandidateId(id.slice(5));
    } else if (id.startsWith('sr:') && composerHere) {
      search.select(id.slice(3));
    } else if (id.startsWith('stop:')) setSelectedStopId(id.slice(5));
    else if (id.startsWith('city:')) {
      const day = days.find((d) => d.cityHint === id.slice(5));
      if (day) onSelect(day.id);
    } else if (id.startsWith('run:')) onSelect(id.slice(4));
    else if (id.startsWith('bead:')) onSelect(id.slice(5));
  };

  return (
    <div className="map-shell">
      <aside className="map-panel">
        <div className="map-panel-body">
          {dockedDay ? (
            <ProposeStopComposer
              day={dockedDay}
              detail={detail}
              days={days}
              candidates={candidates}
              tripId={tripId}
              onClose={gov.close}
              docked
              mode={addMode}
              onModeChange={setAddMode}
              candidateId={addCandidateId}
              onCandidateChange={setAddCandidateId}
              search={search}
              onPreviewChange={setAddPreview}
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
                  <CompactStopList
                    geo={dayGeo}
                    kindLabels={kindLabels}
                    selectedId={selectedStopId}
                    onSelect={setSelectedStopId}
                  />
                </>
              ) : (
                <TripPanel
                  detail={detail}
                  days={days}
                  candidates={candidates}
                  membersById={membersById}
                  onSelectDay={onSelect}
                />
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
          <StopPopover
            geo={dayGeo}
            stop={selectedStop}
            kindLabels={kindLabels}
            candidates={candidates}
            threads={threads}
          />
        )}
      </MapView>
    </div>
  );
}

/* ═══════════════ mobile: full-screen map + bottom sheet ═══════════════ */

/** The collapsed sheet: a 44px grip plus the day header, nothing else. */
const COLLAPSED_SHEET_PX = 140;

/**
 * The sheet's history entry, owned at module scope because it outlives any one
 * mount of the component.
 *
 * The naive version — push on mount, `history.back()` on unmount — is broken by
 * StrictMode, and not artificially: mount / unmount / mount is a real sequence
 * (React does it deliberately in development, and Fast Refresh does it in
 * anger), and `history.back()` delivers its `popstate` *asynchronously*, so the
 * event lands on the listener registered by the second mount and closes the
 * sheet the instant it opens. Counting the mounts and deferring the unwind by a
 * task means a remount simply cancels the teardown, and no spurious popstate is
 * ever generated.
 */
let openSheets = 0;
let unwindTimer = 0;

export function PlanMapOverlay({
  onClose,
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
  const activeDay = active === 'trip' ? null : (days.find((d) => d.id === active) ?? days[0]);
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

  // Featured stop defaults to the day's first; day change re-anchors it.
  const prevActive = useRef(active);
  useEffect(() => {
    if (prevActive.current !== active) {
      prevActive.current = active;
      setSelectedStopId(null);
    }
  }, [active]);
  const selectedStop = dayGeo ? (dayGeo.stops.find((s) => s.id === selectedStopId) ?? dayGeo.stops[0] ?? null) : null;

  // This is a modal surface and was never treated as one: it locked body scroll
  // by hand and stopped there, so it had no role, no name, no focus entry, the
  // bottom-nav links stayed in the tab order behind it, and Escape did nothing
  // because the only Escape handler lived in the desktop shell. The shared hook
  // owns focus entry, the Tab trap, the scroll lock and focus restore; Escape
  // and Back are stacked concerns and stay here.
  const dialogRef = useModalChrome<HTMLDivElement>();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (document.querySelector('.lb-backdrop') || document.querySelector('.gov-backdrop')) return;
      onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Android's Back button must dismiss the sheet rather than leave the trip.
  // The pushed entry carries no URL change, so the router sees the same
  // location on the way back out and re-renders nothing.
  const poppedRef = useRef(false);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    openSheets += 1;
    if (unwindTimer) {
      // A remount caught the teardown mid-air — keep the entry we already have.
      clearTimeout(unwindTimer);
      unwindTimer = 0;
    } else {
      window.history.pushState({ itineraMapSheet: true }, '');
    }
    const onPop = () => {
      poppedRef.current = true;
      onCloseRef.current();
    };
    window.addEventListener('popstate', onPop);
    return () => {
      window.removeEventListener('popstate', onPop);
      openSheets -= 1;
      if (openSheets > 0 || poppedRef.current) return;
      unwindTimer = window.setTimeout(() => {
        unwindTimer = 0;
        // Only unwind our own entry. If the app navigated away while the sheet
        // was open, the top of the stack belongs to the router by now and going
        // back would undo that navigation instead.
        if ((window.history.state as { itineraMapSheet?: boolean } | null)?.itineraMapSheet) window.history.back();
      });
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // How much of the map the sheet is sitting on, in pixels — the one number
  // that drives the sheet's height, where the floating controls park, and how
  // much of the frame fitBounds is allowed to use.
  const overlayRef = useRef<HTMLDivElement>(null);
  const topRef = useRef<HTMLDivElement>(null);
  const [overlayH, setOverlayH] = useState(() => window.innerHeight);
  // Measured, not guessed: the floating chip bar is taller on a touch device
  // (44px chips) than on a trackpad one, and the map has to know.
  const [topH, setTopH] = useState(96);
  useLayoutEffect(() => {
    const el = overlayRef.current;
    const top = topRef.current;
    if (!el || !top) return;
    const ro = new ResizeObserver(() => {
      setOverlayH(el.clientHeight);
      setTopH(top.getBoundingClientRect().bottom - el.getBoundingClientRect().top);
    });
    ro.observe(el);
    ro.observe(top);
    setOverlayH(el.clientHeight);
    setTopH(top.getBoundingClientRect().bottom - el.getBoundingClientRect().top);
    return () => ro.disconnect();
  }, []);
  const sheetH = expanded ? Math.round(overlayH * 0.46) : COLLAPSED_SHEET_PX;

  const markers = useMemo(() => {
    if (dayGeo) return dayMarkers(dayGeo, selectedStop?.id ?? null, showCandidates);
    if (tripGeo) return showCandidates ? [...tripGeo.markers, ...tripGeo.candidateMarkers] : tripGeo.markers;
    return [];
  }, [dayGeo, tripGeo, selectedStop, showCandidates]);
  const routes = useMemo(() => (dayGeo ? dayRoutes(dayGeo) : tripGeo ? tripGeo.routes : []), [dayGeo, tripGeo]);
  const bounds = dayGeo ? dayGeo.bounds : tripGeo!.bounds;
  // Per-edge, in pixels: the floating day chips at the top, the sheet at the
  // bottom, and 64px on the right for the zoom / candidates column stacked
  // above it. A single scalar padding could describe none of them, which is how
  // the day's stops ended up under the chip bar (unclickable) and the whole
  // trip ended up in a 120px strip with half of it behind the sheet. The extra
  // 24px on the top edge is headroom for the spiderfy displacement, which can
  // lift a pin ~20px above where it projected. Memoised because MapView refits
  // whenever this identity changes.
  const padding = useMemo<EdgePadPx>(
    () => ({ top: topH + 24, right: 64, bottom: sheetH + 20, left: 20 }),
    [sheetH, topH],
  );

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
    <div
      className="map-overlay"
      // `--sheet-h` is the single source for the sheet's height, the parking
      // spot of the floating controls, and the map's bottom padding. They used
      // to be three independent 48%/46%/118px guesses, so collapsing the sheet
      // left the zoom buttons floating in empty space and never refit the map.
      style={{ '--sheet-h': `${sheetH}px` } as CSSProperties}
      ref={(node) => {
        overlayRef.current = node;
        dialogRef.current = node;
        // Explicit: a ref callback that returns a value is a React 19 cleanup
        // function, and returning the assignment's result is not one.
      }}
      role="dialog"
      aria-modal="true"
      aria-label="Map"
      tabIndex={-1}
    >
      {/* Ahead of the map on purpose. Everything here is absolutely positioned
          with an explicit z-index, so DOM order costs nothing visually — but it
          decides where focus lands when the sheet opens, and the alternative
          was the first map marker: a node the renderer throws away and rebuilds
          on the next pan, taking the focus ring with it. */}
      <div className="m-top" ref={topRef}>
        <button type="button" className="m-back" onClick={onClose}>
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={2.2}
            strokeLinecap="round"
            aria-hidden
          >
            <path d="M4 6h16" /> <path d="M4 12h16" /> <path d="M4 18h10" />
          </svg>
          Timeline
        </button>
        <MapScrubber days={days} active={active} onSelect={onSelect} />
      </div>

      <MapView
        markers={markers}
        routes={routes}
        bounds={bounds}
        padding={padding}
        className="map-overlay-map"
        onMarkerClick={(id) => {
          if (id.startsWith('stop:')) {
            setSelectedStopId(id.slice(5));
            setExpanded(true);
          } else if (id.startsWith('city:')) {
            const day = days.find((d) => d.cityHint === id.slice(5));
            if (day) onSelect(day.id);
          } else if (id.startsWith('run:')) onSelect(id.slice(4));
          else if (id.startsWith('bead:')) onSelect(id.slice(5));
        }}
      >
        <CandidatesLayerToggle on={showCandidates} onToggle={() => setShowCandidates((v) => !v)} />
      </MapView>

      <div className={`map-sheet${expanded ? '' : ' collapsed'}`}>
        <div
          className="sheet-grip"
          role="button"
          tabIndex={0}
          aria-expanded={expanded}
          aria-label={expanded ? 'Collapse the day list' : 'Expand the day list'}
          onPointerDown={onGripDown}
          onPointerUp={onGripUp}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              setExpanded((v) => !v);
            }
          }}
        >
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
                            <KindGlyph kind={stop.stopKind} />
                            <strong>{place.name}</strong>
                            <span className="ms-kind">{kindLabels[stop.stopKind]}</span>
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
                          <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
                          {place?.name ?? stop.placeId}
                          <span className="ms-kind">{kindLabels[stop.stopKind]}</span>
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
            <TripPanel
              detail={detail}
              days={days}
              candidates={candidates}
              membersById={membersById}
              onSelectDay={onSelect}
            />
          </div>
        )}
      </div>
    </div>
  );
}

/** Floating "open the map" pill — the mobile entry point to the map sheet. */
export function MapPill({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="map-pill" onClick={onClick}>
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <path d="M5 17.5c0-4.5 4.5-3.5 5.5-7.5S16 6 16 6" strokeDasharray="0.1 3.4" />
        <circle cx="5" cy="18.5" r="2.2" fill="currentColor" stroke="none" />
        <circle cx="17.5" cy="5.5" r="2.2" fill="currentColor" stroke="none" />
      </svg>
      Map
    </button>
  );
}
