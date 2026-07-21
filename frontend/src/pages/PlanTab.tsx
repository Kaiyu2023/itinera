import { Fragment } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate } from '../components/hooks';
import type { Leg, PlanDetail, Stop } from '../api/types';

const KIND_COLOR: Record<Stop['stopKind'], string> = {
  visit: 'var(--color-kind-sight)',
  meal: 'var(--color-kind-food)',
  lodging: 'var(--color-kind-lodging)',
  activity: 'var(--color-kind-activity)',
  transit: 'var(--color-kind-transit)',
};

const MODE_ICON: Record<Leg['mode'], string> = { walk: '🚶', transit: '🚃', drive: '🚗', flight: '✈️' };

/** Timeline view of the current plan. The map joins in milestone 2. */
export function PlanTab() {
  const { tripId } = useParams();
  const api = useApi();
  const plan = useQuery({
    queryKey: ['plan', tripId],
    queryFn: () => api.getCurrentPlan(tripId!),
    enabled: !!tripId,
  });

  if (plan.isLoading) return <p className="muted">Loading plan…</p>;
  if (!plan.data) return <p className="muted">No plan yet.</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <p className="muted">
        Plan v{plan.data.plan.version} · {plan.data.days.length} days · {plan.data.stops.length} stops
      </p>
      {plan.data.days.map((day, i) => (
        <DayCard key={day.id} detail={plan.data} dayIndex={i} />
      ))}
    </div>
  );
}

function DayCard({ detail, dayIndex }: { detail: PlanDetail; dayIndex: number }) {
  const day = detail.days[dayIndex];
  const stops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  return (
    <section className="card">
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
        <h2 style={{ fontSize: 'var(--text-lg)' }}>
          Day {dayIndex + 1} · {day.cityHint}
        </h2>
        <span className="muted">{formatDate(day.date)}</span>
        <span className="muted">
          {day.windowStart}–{day.windowEnd}
        </span>
        {feasibility && <span className={`badge ${feasibility.feasibility}`}>{feasibility.feasibility}</span>}
      </div>

      {feasibility && feasibility.notes.length > 0 && (
        <ul className="muted" style={{ margin: 'var(--space-2) 0 0', paddingLeft: 'var(--space-4)' }}>
          {feasibility.notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      )}

      <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-2)' }}>
        {stops.map((stop) => {
          const place = placeById.get(stop.placeId);
          const legIn = detail.legs.find((l) => l.toStopId === stop.id);
          return (
            <Fragment key={stop.id}>
              {legIn && (
                <div className="muted" style={{ paddingLeft: 'var(--space-5)', fontSize: 'var(--text-xs)' }}>
                  {MODE_ICON[legIn.mode]} {legIn.durationMin} min · {(legIn.distanceM / 1000).toFixed(1)} km
                  {legIn.feasibility !== 'ok' && <span className={`badge ${legIn.feasibility}`} style={{ marginLeft: 'var(--space-2)' }}>{legIn.feasibility}</span>}
                  {legIn.feasibilityNote && <span> — {legIn.feasibilityNote}</span>}
                </div>
              )}
              <div style={{ display: 'flex', gap: 'var(--space-3)', alignItems: 'baseline' }}>
                <span
                  style={{
                    width: 10,
                    height: 10,
                    borderRadius: '50%',
                    background: KIND_COLOR[stop.stopKind],
                    flexShrink: 0,
                    position: 'relative',
                    top: -1,
                  }}
                />
                <span style={{ fontVariantNumeric: 'tabular-nums', color: 'var(--color-text-muted)', fontSize: 'var(--text-sm)' }}>
                  {stop.plannedArrival}
                </span>
                <div style={{ flex: 1 }}>
                  <strong>{place?.name ?? stop.placeId}</strong>
                  <span className="muted"> · {stop.durationMin} min</span>
                  {stop.booking && <span className="badge" style={{ marginLeft: 'var(--space-2)' }}>booked</span>}
                  {stop.notes && <p className="muted">{stop.notes}</p>}
                </div>
              </div>
            </Fragment>
          );
        })}
      </div>
    </section>
  );
}
