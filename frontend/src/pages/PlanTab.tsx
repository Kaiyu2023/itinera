import { Fragment, useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDuration, useIsDesktop, useMembers } from '../components/hooks';
import { DaylightStrip } from '../components/DaylightStrip';
import { PlaceThumb } from '../components/PlaceThumb';
import { KIND_COLOR, KIND_LABEL, MODE_ICON } from './planShared';
import { MapPill, PlanMapOverlay, PlanMapShell } from './PlanMap';
import type { MapSelection } from './PlanMap';
import type { Day, PlanDetail, StopKind } from '../api/types';

const VIEW_KEY = 'itinera.planView';
type PlanView = 'timeline' | 'map';

/**
 * The Plan tab. Desktop offers two views of the same plan — the full-width
 * timeline, and the map (timeline panel + map card) — via a segmented toggle;
 * the choice sticks per device. Phones keep the timeline and open the map as
 * a full-screen sheet from the floating pill.
 *
 * Deep links: ?view=map|timeline, ?day=<dayId|trip>, ?stop=<stopId>.
 */
export function PlanTab() {
  const { tripId } = useParams();
  const api = useApi();
  const isDesktop = useIsDesktop();
  const [searchParams] = useSearchParams();

  const plan = useQuery({
    queryKey: ['plan', tripId],
    queryFn: () => api.getCurrentPlan(tripId!),
    enabled: !!tripId,
  });
  // Cache-shared with TripLayout; read here for labels, name, and candidates.
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });
  const threads = useQuery({ queryKey: ['threads', tripId], queryFn: () => api.listThreads(tripId!), enabled: !!tripId });
  const members = useMembers(tripId);

  const [view, setViewState] = useState<PlanView>(() => {
    const fromUrl = searchParams.get('view');
    if (fromUrl === 'map' || fromUrl === 'timeline') return fromUrl;
    return localStorage.getItem(VIEW_KEY) === 'timeline' ? 'timeline' : 'map';
  });
  const setView = (v: PlanView) => {
    setViewState(v);
    localStorage.setItem(VIEW_KEY, v);
  };
  const [active, setActive] = useState<MapSelection | null>(() => searchParams.get('day'));
  const [mapOpen, setMapOpen] = useState(() => searchParams.get('view') === 'map');
  const [initialStopId] = useState(() => searchParams.get('stop'));

  // ?stop= deep link lands on that stop's day
  useEffect(() => {
    if (!initialStopId || !plan.data) return;
    const stop = plan.data.stops.find((s) => s.id === initialStopId);
    if (stop) setActive(stop.dayId);
  }, [initialStopId, plan.data]);

  if (plan.isLoading) return <p className="muted">Loading plan…</p>;
  if (!plan.data) return <p className="muted">No plan yet.</p>;

  const detail = plan.data;
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const activeDay = days.find((d) => d.id === active) ?? days[0];
  const mapActive: MapSelection = active === 'trip' ? 'trip' : activeDay?.id ?? 'trip';
  const kindLabels = { ...KIND_LABEL, ...trip.data?.stopKindLabels };

  const mapProps = {
    detail,
    days,
    kindLabels,
    candidates: candidates.data ?? [],
    membersById: members.byId,
    threads: threads.data ?? [],
    active: mapActive,
    onSelect: setActive,
    initialStopId,
  };

  return (
    <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
      <div className="plan-toolbar">
        <p className="muted">
          Plan v{detail.plan.version} · {days.length} days · {detail.stops.length} stops
        </p>
        {isDesktop && <ViewToggle view={view} onChange={setView} />}
      </div>

      {isDesktop && view === 'map' ? (
        <PlanMapShell {...mapProps} />
      ) : (
        <>
          <div className="day-scrubber" role="tablist" aria-label="Days">
            {days.map((day) => (
              <button
                key={day.id}
                role="tab"
                aria-selected={day.id === activeDay?.id}
                className={`day-chip${day.id === activeDay?.id ? ' active' : ''}`}
                onClick={() => setActive(day.id)}
              >
                {new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' })}
              </button>
            ))}
          </div>
          {activeDay && (
            <DayTimeline detail={detail} day={activeDay} dayIndex={days.indexOf(activeDay)} kindLabels={kindLabels} />
          )}
        </>
      )}

      {!isDesktop && !mapOpen && <MapPill onClick={() => setMapOpen(true)} />}
      {!isDesktop && mapOpen && (
        <PlanMapOverlay {...mapProps} onClose={() => setMapOpen(false)} />
      )}
    </div>
  );
}

function ViewToggle({ view, onChange }: { view: PlanView; onChange: (v: PlanView) => void }) {
  return (
    <div className="seg" role="tablist" aria-label="Plan view">
      <button role="tab" aria-selected={view === 'timeline'} className={view === 'timeline' ? 'active' : ''} onClick={() => onChange('timeline')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" aria-hidden>
          <path d="M4 6h16" /> <path d="M4 12h16" /> <path d="M4 18h10" />
        </svg>
        Timeline
      </button>
      <button role="tab" aria-selected={view === 'map'} className={view === 'map' ? 'active' : ''} onClick={() => onChange('map')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <path d="M9 4L3.5 6v14L9 18l6 2 5.5-2V4L15 6 9 4z" />
          <path d="M9 4v14" />
          <path d="M15 6v14" />
        </svg>
        Map
      </button>
    </div>
  );
}

function DayTimeline({
  detail,
  day,
  dayIndex,
  kindLabels,
}: {
  detail: PlanDetail;
  day: Day;
  dayIndex: number;
  kindLabels: Record<StopKind, string>;
}) {
  const stops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));
  const lodging = stops.find((s) => s.stopKind === 'lodging');
  const lodgingName = lodging ? placeById.get(lodging.placeId)?.name : null;
  const longDate = new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, {
    weekday: 'long',
    month: 'short',
    day: 'numeric',
  });

  return (
    <section style={{ display: 'grid', gap: 'var(--space-3)' }}>
      <div className="day-head">
        <div className="day-numblock">
          <span className="day-eyebrow">Day</span>
          <span className="day-num">{String(dayIndex + 1).padStart(2, '0')}</span>
        </div>
        <div>
          <h2 className="day-city">{day.cityHint}</h2>
          <p className="muted">
            {longDate} · window {day.windowStart}–{day.windowEnd}
            {lodgingName && ` · ${lodgingName}`}
          </p>
        </div>
        {feasibility && (
          <span className={`badge day-verdict ${feasibility.feasibility}`}>
            {feasibility.feasibility} · {feasibility.usedMin} / {feasibility.windowMin} min ·{' '}
            {Math.round((feasibility.usedMin / feasibility.windowMin) * 100)}%
          </span>
        )}
      </div>

      {feasibility && feasibility.notes.length > 0 && (
        <ul className="muted" style={{ margin: 0, paddingLeft: 'var(--space-4)' }}>
          {feasibility.notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      )}

      <DaylightStrip day={day} detail={detail} stops={stops} />

      <div>
        {stops.map((stop) => {
          const place = placeById.get(stop.placeId);
          const legIn = detail.legs.find((l) => l.toStopId === stop.id);
          return (
            <Fragment key={stop.id}>
              {legIn && (
                <div className="tl-row">
                  <div className="tl-time" />
                  <div className="tl-rail" />
                  <div className="leg">
                    <span className={`leg-chip${legIn.feasibility !== 'ok' ? ` ${legIn.feasibility}` : ''}`}>
                      {MODE_ICON[legIn.mode]} {legIn.durationMin} min · {(legIn.distanceM / 1000).toFixed(1)} km
                      {legIn.feasibilityNote && ` — ${legIn.feasibilityNote}`}
                    </span>
                  </div>
                </div>
              )}
              <div className="tl-row">
                <div className="tl-time">{stop.plannedArrival}</div>
                <div className="tl-rail">
                  <span className="tl-node" style={{ '--kind': KIND_COLOR[stop.stopKind] } as React.CSSProperties} />
                </div>
                <div>
                  <article className="stop-card">
                    <div className="stop-head">
                      <strong>{place?.name ?? stop.placeId}</strong>
                      <span className="kind-label" style={{ color: KIND_COLOR[stop.stopKind] }}>
                        {kindLabels[stop.stopKind]}
                      </span>
                      {stop.booking && <span className="badge">booked</span>}
                    </div>
                    <div className="muted">
                      <span className="t-arr">{stop.plannedArrival} · </span>
                      {formatDuration(stop.durationMin)}
                    </div>
                    {stop.notes && <p className="muted">{stop.notes}</p>}
                    {place && <PlaceThumb photos={place.photoUrls} name={place.name} />}
                  </article>
                </div>
              </div>
            </Fragment>
          );
        })}
      </div>
    </section>
  );
}
