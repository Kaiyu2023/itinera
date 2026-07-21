import { Fragment, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate } from '../components/hooks';
import { PlaceThumb } from '../components/PlaceThumb';
import { formatInTz, hhmmToMin, sunTimes } from '../lib/sun';
import type { Day, Leg, PlanDetail, Stop } from '../api/types';

const KIND_COLOR: Record<Stop['stopKind'], string> = {
  visit: 'var(--color-kind-sight)',
  meal: 'var(--color-kind-food)',
  lodging: 'var(--color-kind-lodging)',
  activity: 'var(--color-kind-activity)',
  transit: 'var(--color-kind-transit)',
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
  const [activeDayId, setActiveDayId] = useState<string | null>(null);

  if (plan.isLoading) return <p className="muted">Loading plan…</p>;
  if (!plan.data) return <p className="muted">No plan yet.</p>;

  const detail = plan.data;
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const activeDay = days.find((d) => d.id === activeDayId) ?? days[0];

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
      {activeDay && <DayTimeline detail={detail} day={activeDay} dayIndex={days.indexOf(activeDay)} />}
    </div>
  );
}

function DayTimeline({ detail, day, dayIndex }: { detail: PlanDetail; day: Day; dayIndex: number }) {
  const stops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  return (
    <section style={{ display: 'grid', gap: 'var(--space-3)' }}>
      <div className="day-head">
        <span className="day-num">{String(dayIndex + 1).padStart(2, '0')}</span>
        <div>
          <h2 className="day-city">{day.cityHint}</h2>
          <p className="muted">
            {formatDate(day.date)} · window {day.windowStart}–{day.windowEnd}
          </p>
        </div>
        {feasibility && (
          <span className={`badge ${feasibility.feasibility}`} style={{ marginLeft: 'auto' }}>
            {feasibility.feasibility} · {feasibility.usedMin}/{feasibility.windowMin} min
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
                    <div>
                      <strong>{place?.name ?? stop.placeId}</strong>
                      {stop.booking && (
                        <span className="badge" style={{ marginLeft: 'var(--space-2)' }}>
                          booked
                        </span>
                      )}
                    </div>
                    <div className="muted">
                      <span className="t-arr">{stop.plannedArrival} · </span>
                      {stop.durationMin} min
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

/**
 * Sunrise→sunset within the day's planning window, computed locally (no API).
 * Hidden when the day has no located stops or the sun doesn't rise/set there.
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
  const pct = (min: number) => Math.max(0, Math.min(100, ((min - windowStart) / (windowEnd - windowStart)) * 100));
  const left = pct(hhmmToMin(rise));
  const right = pct(hhmmToMin(set));

  return (
    <div className="daylight" title={`Daylight ${rise}–${set}, shown across the ${day.windowStart}–${day.windowEnd} window`}>
      <div className="track">
        <span className="sun" style={{ left: `${left}%`, width: `${right - left}%` }} />
      </div>
      <div className="labels">
        <span>☀ up {rise}</span>
        <span>🌇 down {set}</span>
      </div>
    </div>
  );
}
