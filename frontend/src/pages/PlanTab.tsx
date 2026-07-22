import { Fragment, useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDuration, useIsDesktop, useMembers } from '../components/hooks';
import { DaylightStrip } from '../components/DaylightStrip';
import { PlaceThumb } from '../components/PlaceThumb';
import { KIND_COLOR, KIND_LABEL, MODE_ICON } from './planShared';
import { MapPill, PlanMapOverlay, PlanMapShell } from './PlanMap';
import type { MapSelection } from './PlanMap';
import { GovModalHost, PlanActionsProvider, usePlanActions, usePlanActionsState } from './PlanGovernance';
import type { GovState } from './PlanGovernance';
import { StopEditor, DayEditor } from './contentEditors';
import type { Day, PlanDetail, Stop, StopKind, Thread } from '../api/types';

/** The open content editor — a stop's details or a day's, edited in place. */
type EditTarget = { kind: 'stop'; stop: Stop } | { kind: 'day'; day: Day };

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
  const threads = useQuery({
    queryKey: ['threads', tripId],
    queryFn: () => api.listThreads(tripId!),
    enabled: !!tripId,
  });
  const members = useMembers(tripId);
  // One governance host for the whole tab — the timeline, the desktop map shell,
  // and the mobile map overlay all drive this single state, so only one modal is
  // ever mounted. The desktop map view docks the add-stop composer instead of
  // showing it modally (dockAddStop below).
  const gov = usePlanActionsState();

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
  // Content-edit surface (immediate, no governance) — a stop's or a day's fields.
  const [editing, setEditing] = useState<EditTarget | null>(null);

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
  const mapActive: MapSelection = active === 'trip' ? 'trip' : (activeDay?.id ?? 'trip');
  const kindLabels = { ...KIND_LABEL, ...trip.data?.stopKindLabels };

  const candidateList = candidates.data ?? [];
  const threadList = threads.data ?? [];
  const mapProps = {
    tripId: tripId!,
    detail,
    days,
    kindLabels,
    candidates: candidateList,
    membersById: members.byId,
    threads: threadList,
    active: mapActive,
    onSelect: setActive,
    initialStopId,
    gov,
  };

  return (
    <PlanActionsProvider actions={gov.actions}>
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
              <DayTimeline
                detail={detail}
                day={activeDay}
                dayIndex={days.indexOf(activeDay)}
                kindLabels={kindLabels}
                threads={threadList}
                onEditStop={(stop) => setEditing({ kind: 'stop', stop })}
                onEditDay={(day) => setEditing({ kind: 'day', day })}
              />
            )}
          </>
        )}

        {!isDesktop && !mapOpen && <MapPill onClick={() => setMapOpen(true)} />}
        {!isDesktop && mapOpen && <PlanMapOverlay {...mapProps} onClose={() => setMapOpen(false)} />}
      </div>

      {/* Deep links (?gov=addStop|change|discuss) open a surface on load. */}
      <PlanGovBootstrap actions={gov.actions} days={days} detail={detail} />
      {/* Deep links (?edit=stop:<id>|day:<id>) open a content editor on load. */}
      <PlanEditBootstrap days={days} detail={detail} onEdit={setEditing} onActivateDay={setActive} />
      {/* Single host: modals/sheets for every view. The desktop map view docks
          the add-stop composer into its panel, so this host skips it there. */}
      <GovModalHost
        action={gov.action}
        close={gov.close}
        dockAddStop={isDesktop && view === 'map'}
        tripId={tripId!}
        detail={detail}
        days={days}
        candidates={candidateList}
        membersById={members.byId}
        threads={threadList}
      />

      {editing?.kind === 'stop' && (
        <StopEditor
          stop={editing.stop}
          placeName={detail.places.find((p) => p.id === editing.stop.placeId)?.name ?? editing.stop.placeId}
          tripId={tripId!}
          onClose={() => setEditing(null)}
        />
      )}
      {editing?.kind === 'day' && (
        <DayEditor
          day={editing.day}
          dayIndex={days.findIndex((d) => d.id === editing.day.id)}
          tripId={tripId!}
          onClose={() => setEditing(null)}
        />
      )}
    </PlanActionsProvider>
  );
}

/** One-shot deep-link opener for content editors: reads `?edit=stop:<id>` or
    `?edit=day:<id>` on mount, opens the editor, and strips the param. */
function PlanEditBootstrap({
  days,
  detail,
  onEdit,
  onActivateDay,
}: {
  days: Day[];
  detail: PlanDetail;
  onEdit: (t: EditTarget) => void;
  /** The timeline shows one day at a time — surface the day being edited. */
  onActivateDay: (dayId: string) => void;
}) {
  const [params, setParams] = useSearchParams();
  const ran = useRef(false);
  useEffect(() => {
    if (ran.current) return;
    ran.current = true;
    const edit = params.get('edit');
    if (!edit) return;
    const [kind, id] = edit.split(':');
    if (kind === 'stop') {
      const stop = detail.stops.find((s) => s.id === id);
      if (stop) {
        onActivateDay(stop.dayId);
        onEdit({ kind: 'stop', stop });
      }
    } else if (kind === 'day') {
      const day = days.find((d) => d.id === id);
      if (day) {
        onActivateDay(day.id);
        onEdit({ kind: 'day', day });
      }
    }
    const next = new URLSearchParams(params);
    next.delete('edit');
    setParams(next, { replace: true });
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  return null;
}

/** One-shot deep-link opener: reads `?gov=` on mount and raises the surface.
    A genuine deep-linking feature (also what the review screenshots drive). */
function PlanGovBootstrap({
  actions,
  days,
  detail,
}: {
  actions: GovState['actions'];
  days: Day[];
  detail: PlanDetail;
}) {
  const [params] = useSearchParams();
  const ran = useRef(false);
  useEffect(() => {
    if (ran.current) return;
    ran.current = true;
    const gov = params.get('gov');
    if (!gov) return;
    if (gov === 'addStop') {
      const day = days.find((d) => d.id === params.get('day')) ?? days[0];
      if (day) actions.proposeStop(day);
    } else if (gov === 'change' || gov === 'discuss') {
      const stop = detail.stops.find((s) => s.id === params.get('stop'));
      if (stop) (gov === 'change' ? actions.proposeChange : actions.discuss)(stop);
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  return null;
}

function ViewToggle({ view, onChange }: { view: PlanView; onChange: (v: PlanView) => void }) {
  return (
    <div className="seg" role="tablist" aria-label="Plan view">
      <button
        role="tab"
        aria-selected={view === 'timeline'}
        className={view === 'timeline' ? 'active' : ''}
        onClick={() => onChange('timeline')}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" aria-hidden>
          <path d="M4 6h16" /> <path d="M4 12h16" /> <path d="M4 18h10" />
        </svg>
        Timeline
      </button>
      <button
        role="tab"
        aria-selected={view === 'map'}
        className={view === 'map' ? 'active' : ''}
        onClick={() => onChange('map')}
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
        >
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
  threads,
  onEditStop,
  onEditDay,
}: {
  detail: PlanDetail;
  day: Day;
  dayIndex: number;
  kindLabels: Record<StopKind, string>;
  threads: Thread[];
  onEditStop: (stop: Stop) => void;
  onEditDay: (day: Day) => void;
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
        <button
          type="button"
          className="edit-ghost day-edit"
          onClick={() => onEditDay(day)}
          aria-label={`Edit Day ${dayIndex + 1} details`}
          title="Edit day details"
        >
          ✎
        </button>
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
                    <TimelineStopActions stop={stop} threads={threads} onEdit={() => onEditStop(stop)} />
                  </article>
                </div>
              </div>
            </Fragment>
          );
        })}
        <div className="tl-row">
          <div className="tl-time" />
          <div className="tl-rail" />
          <TimelineProposeStop day={day} />
        </div>
      </div>
    </section>
  );
}

/** Quiet ghost actions on a timeline stop card — the same Discuss / Propose
    change the map popover offers, so both views reach governance the same way. */
function TimelineStopActions({ stop, threads, onEdit }: { stop: Stop; threads: Thread[]; onEdit: () => void }) {
  const actions = usePlanActions();
  const thread = threads.find((t) => t.anchor.kind === 'stop' && t.anchor.stopId === stop.id);
  return (
    <div className="stop-actions">
      <button type="button" className="b" onClick={() => actions.discuss(stop)}>
        💬 Discuss{thread ? ` · ${thread.commentCount}` : ''}
      </button>
      <button type="button" className="b" onClick={() => actions.proposeChange(stop)}>
        ✎ Propose change
      </button>
      <span className="sa-spacer" />
      <button
        type="button"
        className="b edit-ghost"
        onClick={onEdit}
        aria-label={`Edit details for ${stop.plannedArrival} stop`}
        title="Edit details"
      >
        ✎ Edit details
      </button>
    </div>
  );
}

/** "＋ Propose a stop on this day" — matches the map sheet's entry point. */
function TimelineProposeStop({ day }: { day: Day }) {
  const actions = usePlanActions();
  return (
    <button type="button" className="ghost-btn" onClick={() => actions.proposeStop(day)}>
      ＋ Propose a stop on this day
    </button>
  );
}
