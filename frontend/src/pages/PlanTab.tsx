import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { useIsDesktop, useMembers } from '../components/hooks';
import { DayCanvas } from '../components/DayCanvas';
import { KindGlyph } from '../components/KindGlyph';
import { PlaceGuide } from '../components/PlaceGuide';
import { PlacePhotoBanner } from '../components/PlacePhotoBanner';
import { SheetModal } from '../components/SheetModal';
import { TripRibbon } from '../components/TripRibbon';
import { MoonGlyph, SunGlyph, WeatherGlyph } from '../components/SkyGlyph';
import { useI18n } from '../i18n';
import { formatPlanDuration } from '../i18n/messages.plan';
import { daySky } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { useTripWeather } from '../lib/weather';
import type { DayWeather } from '../lib/weather';
import { MapPill, PlanMapOverlay, PlanMapShell } from './PlanMap';
import type { MapSelection } from './PlanMap';
import { GovModalHost, PlanActionsProvider, usePlanActions, usePlanActionsState } from './PlanGovernance';
import type { GovState } from './PlanGovernance';
import { StopEditor, DayEditor } from './contentEditors';
import type { Day, Place, PlanDetail, Stop, StopKind, Thread } from '../api/types';

/** The open content editor — a stop's details or a day's, edited in place. */
type EditTarget = { kind: 'stop'; stop: Stop } | { kind: 'day'; day: Day };

const VIEW_KEY = 'itinera.planView';
type PlanView = 'timeline' | 'map';

const FEASIBILITY_KEY = {
  ok: 'plan.feasibility.ok',
  tight: 'plan.feasibility.tight',
  unreasonable: 'plan.feasibility.unreasonable',
  impossible: 'plan.feasibility.impossible',
} as const;

const WEATHER_KEY = {
  clear: 'plan.weather.clear',
  partly: 'plan.weather.partly',
  cloud: 'plan.weather.cloud',
  fog: 'plan.weather.fog',
  drizzle: 'plan.weather.drizzle',
  rain: 'plan.weather.rain',
  snow: 'plan.weather.snow',
  storm: 'plan.weather.storm',
} as const;

/**
 * The Plan tab. Desktop offers two views of the same plan — the full-width
 * timeline, and the map (timeline panel + map card) — via a segmented toggle;
 * the choice sticks per device. Phones keep the timeline and open the map as
 * a full-screen sheet from the floating pill.
 *
 * Deep links: ?view=map|timeline, ?day=<dayId|trip>, ?stop=<stopId>.
 */
export function PlanTab() {
  const { t, formatDate } = useI18n();
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
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
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
  // Environment, fetched not stored — same standing as sunrise/sunset. Called
  // above the early returns because it is a hook, and it is safe to: it is
  // disabled until the plan lands and resolves to `{}` on any failure.
  const tripWeather = useTripWeather(tripId, plan.data?.days ?? [], plan.data);
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
  // A generic `?stop=` also accompanies map and governance deep links. On a
  // phone those links must not bootstrap a second timeline detail sheet behind
  // the surface the URL actually requested.
  const [suppressInitialTimelineDetail] = useState(
    () => searchParams.get('view') === 'map' || searchParams.has('gov') || searchParams.has('edit'),
  );
  // Content-edit surface (immediate, no governance) — a stop's or a day's fields.
  const [editing, setEditing] = useState<EditTarget | null>(null);

  // ?stop= deep link lands on that stop's day
  useEffect(() => {
    if (!initialStopId || !plan.data) return;
    const stop = plan.data.stops.find((s) => s.id === initialStopId);
    if (stop) setActive(stop.dayId);
  }, [initialStopId, plan.data]);

  if (plan.isLoading || trip.isLoading || me.isLoading) return <p className="muted">{t('plan.loading')}</p>;
  // `?view=map` used to land here and render four bare words — no map, no
  // toolbar, no way forward, and the requested view silently discarded. A
  // planless trip now teaches the actual first-plan flow: save a place, then
  // propose it. The first proposal supplies the city/timezone needed to mint
  // the dated Day skeleton before the governance composer opens.
  if (!plan.data) {
    const ideaCount = candidates.data?.filter((candidate) => candidate.status === 'shortlisted').length ?? 0;
    return (
      <section className="plan-zero" aria-labelledby="plan-zero-title">
        <div className="plan-zero-icon" aria-hidden>
          <svg viewBox="0 0 48 48">
            <path d="M10 35c1-12 10-5 13-17s12-7 15-12" />
            <circle cx="10" cy="36" r="4" />
            <circle cx="38" cy="7" r="4" />
          </svg>
        </div>
        <div className="plan-zero-copy">
          <span className="eyebrow">{t('plan.empty.eyebrow')}</span>
          <h2 id="plan-zero-title">{t('plan.empty.title')}</h2>
          <p>{t('plan.empty.body')}</p>
        </div>
        <ol className="plan-zero-steps">
          <li>
            <span>1</span>
            {t('plan.empty.stepIdea')}
          </li>
          <li>
            <span>2</span>
            {t('plan.empty.stepPropose')}
          </li>
          <li>
            <span>3</span>
            {t('plan.empty.stepRoute')}
          </li>
        </ol>
        <div className="plan-zero-actions">
          <Link className="btn accent" to={`/trips/${tripId}/candidates${ideaCount === 0 ? '?cand=new' : ''}`}>
            {t(ideaCount === 0 ? 'plan.empty.addIdea' : 'plan.empty.chooseIdea')}
          </Link>
          {ideaCount > 0 && <span className="hint">{t('plan.empty.ready', { count: ideaCount })}</span>}
        </div>
      </section>
    );
  }

  const detail = plan.data;
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const isLeader = !!trip.data?.members.some((member) => member.userId === me.data?.id && member.role === 'leader');
  const activeDay = days.find((d) => d.id === active) ?? days[0];
  // Purely additive: absent, offline or slow, everything below renders the same.
  const weather = tripWeather;
  const mapActive: MapSelection = active === 'trip' ? 'trip' : (activeDay?.id ?? 'trip');
  const kindLabels = {
    visit: t('plan.kind.visit'),
    meal: t('plan.kind.meal'),
    lodging: t('plan.kind.lodging'),
    activity: t('plan.kind.activity'),
    transit: t('plan.kind.transit'),
    ...trip.data?.stopKindLabels,
  };

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
    isLeader,
  };

  return (
    <PlanActionsProvider actions={gov.actions}>
      <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
        <div className="plan-toolbar">
          <p className="muted">
            {t('plan.summary', { version: detail.plan.version, days: days.length, stops: detail.stops.length })}
          </p>
          {isDesktop && <ViewToggle view={view} onChange={setView} />}
        </div>

        {isDesktop && view === 'map' ? (
          <PlanMapShell {...mapProps} />
        ) : (
          <>
            <TripRibbon
              days={days}
              detail={detail}
              kindLabels={kindLabels}
              weather={weather}
              active={activeDay?.id ?? null}
              onSelect={setActive}
            />
            <div className="day-scrubber" role="tablist" aria-label={t('plan.days')}>
              {days.map((day) => (
                <button
                  key={day.id}
                  role="tab"
                  aria-selected={day.id === activeDay?.id}
                  className={`day-chip${day.id === activeDay?.id ? ' active' : ''}`}
                  onClick={() => setActive(day.id)}
                >
                  {formatDate(day.date, { weekday: 'short', day: 'numeric' })}
                </button>
              ))}
            </div>
            {activeDay && (
              <DayTimeline
                /* Remounts per day, which is what resets the open stop. Without
                   it the selection stays pointed at the previous day's stop and
                   the new day opens with nothing expanded. */
                key={activeDay.id}
                detail={detail}
                day={activeDay}
                dayIndex={days.indexOf(activeDay)}
                kindLabels={kindLabels}
                weather={weather[activeDay.id]}
                threads={threadList}
                initialStopId={isDesktop || !suppressInitialTimelineDetail ? initialStopId : null}
                selectFirstStop={isDesktop}
                detailsBlocked={!isDesktop && (mapOpen || gov.action !== null || editing !== null)}
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
        isLeader={isLeader}
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
  const { t } = useI18n();
  return (
    <div className="seg" role="tablist" aria-label={t('plan.view.label')}>
      <button
        role="tab"
        aria-selected={view === 'timeline'}
        className={view === 'timeline' ? 'active' : ''}
        onClick={() => onChange('timeline')}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" aria-hidden>
          <path d="M4 6h16" /> <path d="M4 12h16" /> <path d="M4 18h10" />
        </svg>
        {t('plan.view.timeline')}
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
        {t('plan.view.map')}
      </button>
    </div>
  );
}

function DayTimeline({
  detail,
  day,
  dayIndex,
  kindLabels,
  weather,
  threads,
  initialStopId,
  selectFirstStop,
  detailsBlocked,
  onEditStop,
  onEditDay,
}: {
  detail: PlanDetail;
  day: Day;
  dayIndex: number;
  kindLabels: Record<StopKind, string>;
  weather: DayWeather | undefined;
  threads: Thread[];
  initialStopId: string | null;
  selectFirstStop: boolean;
  detailsBlocked: boolean;
  onEditStop: (stop: Stop) => void;
  onEditDay: (day: Day) => void;
}) {
  const { t, formatDate } = useI18n();
  const duration = (minutes: number) => formatPlanDuration(minutes, t);
  const actions = usePlanActions();
  const stops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const sky = daySky(day, detail, stops);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));
  const lodging = stops.find((s) => s.stopKind === 'lodging');
  const lodgingName = lodging ? placeById.get(lodging.placeId)?.name : null;
  const longDate = formatDate(day.date, {
    weekday: 'long',
    month: 'short',
    day: 'numeric',
  });

  // A deep link always selects its stop. Wide layouts also start with the first
  // stop selected because the inspector has a dedicated column; phones start
  // closed so the sticky detail dock never covers the clock before the user
  // asks for it.
  const defaultStopId = stops.some((stop) => stop.id === initialStopId) ? initialStopId : (stops[0]?.id ?? null);
  const [selectedStopId, setSelectedStopId] = useState<string | null>(selectFirstStop ? defaultStopId : initialStopId);
  const previousDesktop = useRef(selectFirstStop);
  useEffect(() => {
    if (previousDesktop.current === selectFirstStop) return;
    previousDesktop.current = selectFirstStop;
    // Crossing into the desktop layout keeps an explicit selection when there
    // is one; crossing to a sheet layout starts closed.
    setSelectedStopId((current) => (selectFirstStop ? (current ?? defaultStopId) : null));
  }, [defaultStopId, selectFirstStop]);
  useEffect(() => {
    if (detailsBlocked) setSelectedStopId(null);
  }, [detailsBlocked]);
  const selectedStop = stops.find((stop) => stop.id === selectedStopId) ?? null;
  const selectedPlace = selectedStop ? (placeById.get(selectedStop.placeId) ?? null) : null;
  const closeInspector = () => {
    const returnTarget = selectedStopId ? document.getElementById(`timeline-stop-${selectedStopId}`) : null;
    setSelectedStopId(null);
    requestAnimationFrame(() => returnTarget?.focus());
  };

  return (
    <section className="dayview" style={{ display: 'grid', gap: 'var(--space-3)' }}>
      <div className="day-head">
        <div className="day-numblock">
          <span className="day-eyebrow">{t('plan.day')}</span>
          <span className="day-num">{String(dayIndex + 1).padStart(2, '0')}</span>
        </div>
        <div>
          <h2 className="day-city">{day.cityHint}</h2>
          <p className="muted">
            {longDate} · {t('plan.day.window', { start: day.windowStart, end: day.windowEnd })}
            {lodgingName && ` · ${lodgingName}`}
          </p>
          {/* Both horizons, always — the canvas can only mark one that falls
              inside the planning window, and in a November itinerary the sun is
              usually up before the window opens, so the day appeared to have an
              end and no beginning. */}
          {sky && (
            <p className="day-sun">
              <SunGlyph label={t('plan.day.sunrise')} />
              {sky.rise}
              <span className="dash" aria-hidden>
                –
              </span>
              <MoonGlyph label={t('plan.day.sunset')} />
              {sky.set}
              <em>{t('plan.day.daylight', { duration: duration(sky.setMin - sky.riseMin) })}</em>
            </p>
          )}
          {weather && <DayWeatherChip weather={weather} />}
        </div>
        {/* Only a problem earns a badge. A day that fits says nothing — the
            column already shows the slack, so "OK · 62%" was noise competing
            with the one signal that matters. */}
        <div className="day-head-actions">
          {feasibility && feasibility.feasibility !== 'ok' && (
            <span className={`badge day-verdict ${feasibility.feasibility}`}>
              {t(FEASIBILITY_KEY[feasibility.feasibility])}
            </span>
          )}
          <button type="button" className="day-add" onClick={() => actions.proposeStop(day)}>
            {t('plan.day.addStop')}
          </button>
          <button
            type="button"
            className="edit-ghost day-edit"
            onClick={() => onEditDay(day)}
            aria-label={t('plan.day.edit', { day: dayIndex + 1 })}
            title={t('plan.day.editTitle')}
          >
            ✎
          </button>
        </div>
      </div>

      {feasibility && feasibility.feasibility !== 'ok' && feasibility.notes.length > 0 && (
        <ul className="day-notes">
          {feasibility.notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      )}

      <div className="day-plan-layout">
        <DayCanvas
          day={day}
          detail={detail}
          stops={stops}
          kindLabels={kindLabels}
          hourHeight={96}
          selectedStopId={selectedStopId}
          onSelectStop={setSelectedStopId}
          onAddStop={(initialSlot) => actions.proposeStop(day, initialSlot)}
        />
        {selectFirstStop && (
          <TimelineStopInspector
            stop={selectedStop}
            place={selectedPlace}
            kindLabels={kindLabels}
            sky={sky}
            threads={threads}
            onEdit={selectedStop ? () => onEditStop(selectedStop) : undefined}
            onClose={closeInspector}
          />
        )}
        {!selectFirstStop && !detailsBlocked && selectedStop && selectedPlace && (
          <TimelineStopSheet
            stop={selectedStop}
            place={selectedPlace}
            kindLabels={kindLabels}
            sky={sky}
            threads={threads}
            onEdit={() => onEditStop(selectedStop)}
            onClose={closeInspector}
          />
        )}
      </div>
    </section>
  );
}

/**
 * The day's weather, and — the part that matters — which kind of claim it is.
 *
 * Every trip in this app is months out, so nobody has a forecast for it. What
 * the `typical` variant says is "this is what this week actually did in each of
 * the last four years", which is a genuinely useful thing to pack against and a
 * dishonest thing to print in the same ink as a forecast. Hence the dotted rule
 * under it and the word.
 */
function DayWeatherChip({ weather }: { weather: DayWeather }) {
  const { t } = useI18n();
  const forecast = weather.source === 'forecast';
  return (
    <p
      className={`day-wx ${weather.source}`}
      title={
        forecast
          ? t('plan.weather.forecastTitle', { chance: weather.wetChance })
          : t('plan.weather.typicalTitle', {
              from: weather.years?.[0] ?? '',
              to: weather.years?.[1] ?? '',
              chance: weather.wetChance,
            })
      }
    >
      <WeatherGlyph condition={weather.condition} label={t(WEATHER_KEY[weather.condition])} />
      <b>
        {weather.tempMax}° / {weather.tempMin}°
      </b>
      <span>{t(WEATHER_KEY[weather.condition])}</span>
      <em>{t(forecast ? 'plan.weather.forecast' : 'plan.weather.typical')}</em>
      {weather.wetChance >= 30 && <span className="wet">{t('plan.weather.wet', { chance: weather.wetChance })}</span>}
    </p>
  );
}

function TimelineStopInspector({
  stop,
  place,
  kindLabels,
  sky,
  threads,
  onEdit,
  onClose,
}: {
  stop: Stop | null;
  place: Place | null;
  kindLabels: Record<StopKind, string>;
  sky: ReturnType<typeof daySky>;
  threads: Thread[];
  onEdit?: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  if (!stop) {
    return (
      <aside className="timeline-inspector empty" aria-label={t('plan.stop.selectedDetails')}>
        <p>{t('plan.stop.selectHint')}</p>
      </aside>
    );
  }

  const start = hhmmToMin(stop.plannedArrival);
  const end = start + stop.durationMin;
  const crossesSunset = !!sky && start < sky.setMin && end > sky.setMin;
  const afterDark = !!sky && start >= sky.setMin;
  return (
    <aside
      id={`timeline-stop-details-${stop.id}`}
      className="timeline-inspector"
      aria-label={t('plan.stop.selectedDetails')}
    >
      <button
        type="button"
        className="ti-dismiss"
        onClick={onClose}
        aria-label={t('plan.stop.closeDetails', { place: place?.name ?? stop.placeId })}
      >
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
      <div className="stop-detail-scroll">
        {place ? (
          <TimelineStopDetail
            stop={stop}
            place={place}
            kindLabels={kindLabels}
            afterDark={afterDark}
            crossesSunset={crossesSunset}
            sunset={sky?.set}
          />
        ) : (
          <h3>{stop.placeId}</h3>
        )}
      </div>
      <TimelineStopActions stop={stop} threads={threads} onEdit={onEdit ?? (() => {})} />
    </aside>
  );
}

function TimelineStopSheet({
  stop,
  place,
  kindLabels,
  sky,
  threads,
  onEdit,
  onClose,
}: {
  stop: Stop;
  place: Place;
  kindLabels: Record<StopKind, string>;
  sky: ReturnType<typeof daySky>;
  threads: Thread[];
  onEdit: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const start = hhmmToMin(stop.plannedArrival);
  const end = start + stop.durationMin;
  const crossesSunset = !!sky && start < sky.setMin && end > sky.setMin;
  const afterDark = !!sky && start >= sky.setMin;
  const dismissThen = (action: () => void) => {
    onClose();
    requestAnimationFrame(action);
  };

  return (
    <SheetModal onClose={onClose}>
      <div
        id={`timeline-stop-details-${stop.id}`}
        className="exp-modal stop-detail-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`stop-detail-title-${stop.id}`}
      >
        <button
          type="button"
          className="x stop-detail-close"
          onClick={onClose}
          aria-label={t('plan.stop.closeDetails', { place: place.name })}
        >
          ×
        </button>
        <div className="stop-detail-scroll">
          <TimelineStopDetail
            stop={stop}
            place={place}
            kindLabels={kindLabels}
            afterDark={afterDark}
            crossesSunset={crossesSunset}
            sunset={sky?.set}
            headingId={`stop-detail-title-${stop.id}`}
          />
        </div>
        <TimelineStopActions stop={stop} threads={threads} onEdit={() => dismissThen(onEdit)} beforeAction={onClose} />
      </div>
    </SheetModal>
  );
}

function TimelineStopDetail({
  stop,
  place,
  kindLabels,
  afterDark,
  crossesSunset,
  sunset,
  headingId,
}: {
  stop: Stop;
  place: Place;
  kindLabels: Record<StopKind, string>;
  afterDark: boolean;
  crossesSunset: boolean;
  sunset?: string;
  headingId?: string;
}) {
  const { t } = useI18n();
  const duration = (minutes: number) => formatPlanDuration(minutes, t);
  return (
    <div className="stop-detail-content">
      <PlacePhotoBanner place={place} />
      <div className="stop-detail-copy">
        <div className="ti-heading">
          <span className="ti-kind">
            <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
            {kindLabels[stop.stopKind]}
          </span>
          <h3 id={headingId}>{place.name}</h3>
          {stop.booking && <span className="badge">{t('plan.stop.booked')}</span>}
        </div>
        <p className="ti-meta">
          <strong>{stop.plannedArrival}</strong>
          <span>{duration(stop.durationMin)}</span>
          {place.rating != null && <span>★ {place.rating.toFixed(1)}</span>}
          {(afterDark || crossesSunset) && (
            <span className="ti-dark">
              <MoonGlyph />
              {crossesSunset ? t('plan.day.sunsetAt', { time: sunset ?? '' }) : t('plan.day.afterDark')}
            </span>
          )}
        </p>
        <PlaceGuide
          place={place}
          tripContext={stop.notes ? <p>{stop.notes}</p> : undefined}
          contextLabel={t('plan.stop.tripNote')}
          variant="full"
        />
        {stop.booking && (
          <section className="ti-booking" aria-label={t('plan.stop.bookingDetails')}>
            <span>{t('plan.stop.booking')}</span>
            <strong>{stop.booking.ref}</strong>
            {stop.booking.url && (
              <a href={stop.booking.url} target="_blank" rel="noreferrer">
                {t('plan.stop.openBooking')}
              </a>
            )}
          </section>
        )}
      </div>
    </div>
  );
}

/** Governance actions live in the selected-stop inspector, outside the clock. */
function TimelineStopActions({
  stop,
  threads,
  onEdit,
  beforeAction,
}: {
  stop: Stop;
  threads: Thread[];
  onEdit: () => void;
  beforeAction?: () => void;
}) {
  const { t } = useI18n();
  const actions = usePlanActions();
  const thread = threads.find((t) => t.anchor.kind === 'stop' && t.anchor.stopId === stop.id);
  const run = (action: () => void) => {
    beforeAction?.();
    requestAnimationFrame(action);
  };
  return (
    <div className="stop-actions ti-actions">
      <button type="button" className="b primary" onClick={() => run(() => actions.proposeChange(stop))}>
        {t('plan.stop.proposeChange')}
      </button>
      <button type="button" className="b" onClick={() => run(() => actions.discuss(stop))}>
        {thread ? t('plan.stop.discussCount', { count: thread.commentCount }) : t('plan.stop.discuss')}
      </button>
      <details className="stop-more">
        <summary aria-label={t('plan.stop.moreActions')} title={t('plan.stop.moreActionsTitle')}>
          ⋯
        </summary>
        <button type="button" onClick={onEdit} aria-label={t('plan.stop.editDetailsAt', { time: stop.plannedArrival })}>
          {t('plan.stop.editDetails')}
        </button>
      </details>
    </div>
  );
}
