import { Fragment, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDuration } from '../components/hooks';
import { PlaceThumb } from '../components/PlaceThumb';
import { formatInTz, hhmmToMin, sunTimes } from '../lib/sun';
import type { Day, Leg, PlanDetail, Stop, StopKind } from '../api/types';

const KIND_COLOR: Record<Stop['stopKind'], string> = {
  visit: 'var(--color-kind-sight)',
  meal: 'var(--color-kind-food)',
  lodging: 'var(--color-kind-lodging)',
  activity: 'var(--color-kind-activity)',
  transit: 'var(--color-kind-transit)',
};

/** Built-in card labels; trips override per kind via Trip.stopKindLabels. */
const KIND_LABEL: Record<StopKind, string> = {
  visit: 'visit',
  meal: 'meal',
  lodging: 'lodging',
  activity: 'activity',
  transit: 'transit',
};

const MODE_ICON: Record<Leg['mode'], string> = { walk: '🚶', transit: '🚃', drive: '🚗', flight: '✈️' };

/** One day at a time, picked from the scrubber. The map joins in milestone 2. */
export function PlanTab() {
  const { tripId } = useParams();
  const api = useApi();
  const plan = useQuery({
    queryKey: ['plan', tripId],
    queryFn: () => api.getCurrentPlan(tripId!),
    enabled: !!tripId,
  });
  // Cache-shared with TripLayout; only read here for the stop-kind labels.
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const [activeDayId, setActiveDayId] = useState<string | null>(null);

  if (plan.isLoading) return <p className="muted">Loading plan…</p>;
  if (!plan.data) return <p className="muted">No plan yet.</p>;

  const detail = plan.data;
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const activeDay = days.find((d) => d.id === activeDayId) ?? days[0];
  const kindLabels = { ...KIND_LABEL, ...trip.data?.stopKindLabels };

  return (
    <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
      <p className="muted">
        Plan v{detail.plan.version} · {days.length} days · {detail.stops.length} stops
      </p>
      <div className="day-scrubber" role="tablist" aria-label="Days">
        {days.map((day) => (
          <button
            key={day.id}
            role="tab"
            aria-selected={day.id === activeDay?.id}
            className={`day-chip${day.id === activeDay?.id ? ' active' : ''}`}
            onClick={() => setActiveDayId(day.id)}
          >
            {new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' })}
          </button>
        ))}
      </div>
      {activeDay && (
        <DayTimeline detail={detail} day={activeDay} dayIndex={days.indexOf(activeDay)} kindLabels={kindLabels} />
      )}
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

/** Sky colors for the daylight strip — dawn/dusk glow between day and night. */
const SKY = { night: '#353c66', plum: '#8a6488', glow: '#e8935a', gold: '#f4cf7a', day: '#dce8f2' };

/**
 * The day's window painted as a sky: pale blue daylight, a golden-hour ramp
 * into dusk, indigo night, and a tick at the sunset moment. Computed locally
 * (no API). Hidden when the day has no located stops or no sunrise/sunset.
 */
function DaylightStrip({ day, detail, stops }: { day: Day; detail: PlanDetail; stops: Stop[] }) {
  const anchor = stops.map((s) => detail.places.find((p) => p.id === s.placeId)).find(Boolean);
  if (!anchor) return null;
  const sun = sunTimes(day.date, anchor.lat, anchor.lng);
  if (!sun) return null;

  const rise = formatInTz(sun.sunrise, day.tz);
  const set = formatInTz(sun.sunset, day.tz);
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const span = windowEnd - windowStart;
  // Unclamped window fractions — negative/over-100 means outside the window.
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

  return (
    <div className="daylight" title={`Daylight ${rise}–${set}, shown across the ${day.windowStart}–${day.windowEnd} window`}>
      <div className="track" style={{ background: `linear-gradient(90deg, ${g.join(', ')})` }}>
        {setAt > 1.5 && setAt < 98.5 && <span className="tick" style={{ left: `${setAt}%` }} />}
      </div>
      <div className="labels">
        <span>{day.windowStart}</span>
        {setAt > 0 && setAt < 100 ? (
          <span className="sun-label">sunset {set} ☀ ↓</span>
        ) : riseAt > 0 && riseAt < 100 ? (
          <span className="sun-label">sunrise {rise} ☀ ↑</span>
        ) : null}
        <span>{day.windowEnd}</span>
      </div>
    </div>
  );
}
