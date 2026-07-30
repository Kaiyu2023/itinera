import { useEffect, useRef } from 'react';
import { hhmmToMin } from '../lib/sun';
import { formatDuration } from './hooks';
import { KindGlyph } from './KindGlyph';
import type { Day, PlanDetail, StopKind } from '../api/types';

/**
 * The whole trip as one continuous line — the view the app did not have.
 *
 * Sequence is preserved and geography is sacrificed: stops are stations, legs
 * are the track between them, everything sized by how long it actually takes.
 * The lineage is literal — the Roman *itineraria* the app is named after were
 * exactly this, as were the Peutinger Table and AAA's TripTiks. It is the right
 * diagram for "a route through time", and nothing in this product category
 * draws it.
 *
 * Where the day canvas answers "what does Thursday feel like", this answers
 * "what does the week feel like" — which days are dense, which are a single
 * long haul, where the travel actually goes.
 */

/** Horizontal scale. Small enough that a week fits a desktop width. */
const PX_PER_MIN = 0.26;
/** A stop narrower than this gets no label; the line still shows its extent. */
const LABEL_MIN_PX = 54;

const MODE_CLASS: Record<string, string> = {
  walk: 'walk',
  transit: 'transit',
  drive: 'drive',
  flight: 'flight',
};

export function TripRibbon({
  days,
  detail,
  kindLabels,
  active,
  onSelect,
}: {
  days: Day[];
  detail: PlanDetail;
  kindLabels: Record<StopKind, string>;
  active: string | null;
  onSelect: (dayId: string) => void;
}) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  // Keep the selected day in view when it changes from elsewhere (the scrubber,
  // a deep link) — otherwise the ribbon silently disagrees with the canvas.
  useEffect(() => {
    const el = trackRef.current?.querySelector<HTMLElement>(`[data-day="${active}"]`);
    el?.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' });
  }, [active]);

  return (
    <section className="ribbon" aria-label="Whole trip at a glance">
      <div className="rb-track" ref={trackRef}>
        {days.map((day, dayIndex) => {
          const stops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
          const feas = detail.dayFeasibility.find((f) => f.dayId === day.id);
          const parts: React.ReactNode[] = [];
          let dayMin = 0;

          stops.forEach((stop, i) => {
            const leg = detail.legs.find((l) => l.toStopId === stop.id);
            if (leg && i > 0) {
              const w = leg.durationMin * PX_PER_MIN;
              dayMin += leg.durationMin;
              parts.push(
                <span
                  key={`${stop.id}-leg`}
                  className={`rb-leg ${MODE_CLASS[leg.mode] ?? 'transit'}${leg.feasibility !== 'ok' ? ' warn' : ''}`}
                  style={{ width: `${Math.max(6, w)}px` }}
                  title={`${leg.mode} ${leg.durationMin} min${leg.feasibilityNote ? ` — ${leg.feasibilityNote}` : ''}`}
                />,
              );
            }
            const w = stop.durationMin * PX_PER_MIN;
            dayMin += stop.durationMin;
            const place = placeById.get(stop.placeId);
            parts.push(
              <span
                key={stop.id}
                className="rb-stop"
                style={{ width: `${Math.max(10, w)}px` }}
                title={`${place?.name ?? stop.placeId} · ${stop.plannedArrival} · ${formatDuration(stop.durationMin)}`}
              >
                <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
                {w >= LABEL_MIN_PX && <i className="rb-name">{place?.name ?? stop.placeId}</i>}
              </span>,
            );
          });

          const start = stops[0]?.plannedArrival ?? day.windowStart;
          const last = stops[stops.length - 1];
          const end = last
            ? `${String(Math.floor((hhmmToMin(last.plannedArrival) + last.durationMin) / 60)).padStart(2, '0')}:${String(
                (hhmmToMin(last.plannedArrival) + last.durationMin) % 60,
              ).padStart(2, '0')}`
            : day.windowEnd;

          return (
            <button
              key={day.id}
              type="button"
              data-day={day.id}
              className={`rb-day${day.id === active ? ' active' : ''}`}
              aria-pressed={day.id === active}
              onClick={() => onSelect(day.id)}
              style={{ minWidth: `${Math.max(96, dayMin * PX_PER_MIN)}px` }}
            >
              <span className="rb-daynum">
                {String(dayIndex + 1).padStart(2, '0')}
                <i>{day.cityHint}</i>
              </span>
              <span className="rb-line">{parts}</span>
              <span className="rb-foot">
                {start}–{end}
                {feas && feas.feasibility !== 'ok' && (
                  <em className={`rb-flag ${feas.feasibility}`}>{feas.feasibility}</em>
                )}
              </span>
            </button>
          );
        })}
      </div>
    </section>
  );
}
