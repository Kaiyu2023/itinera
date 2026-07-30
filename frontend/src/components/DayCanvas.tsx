import { Fragment } from 'react';
import type { CSSProperties } from 'react';
import { daySky } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { formatDuration } from './hooks';
import { KindGlyph } from './KindGlyph';
import { MODE_ICON } from '../pages/planShared';
import type { Day, PlanDetail, Stop, StopKind } from '../api/types';

/**
 * A day drawn as physical space: one minute of the day is a fixed number of
 * pixels, top to bottom, with the sky painted behind the column.
 *
 * The point is to stop *describing* the day and start *depicting* it. In the
 * old timeline a four-hour temple visit and a five-minute walk were rows of
 * identical height, and the day was "87% full" because a badge said so. Here
 * a long visit is a long block, a tight day is a column with no gaps, a day
 * that does not fit visibly overflows its window, and the note that
 * "Arashiyama at 14:45 leaves little daylight for the grove" stops being prose
 * because the block sits below the sunset line.
 *
 * Blocks are positioned absolutely and do NOT grow to fit their content: the
 * scale has to mean something, so a 20-minute stop is a 20-minute stop and its
 * detail degrades instead. `.sz-*` classes let the content adapt to the room
 * the clock actually gave it.
 */

/** Pixel heights below which a block cannot hold a given tier of detail. */
const SIZE_FULL = 78; // name + meta + note
const SIZE_MEDIUM = 46; // name + meta
/** Above this a block has room to spare, so the place's photo fills it. */
const SIZE_PHOTO = 150;

function sizeClass(px: number): string {
  if (px >= SIZE_FULL) return 'sz-full';
  if (px >= SIZE_MEDIUM) return 'sz-med';
  return 'sz-min';
}

export interface DayCanvasProps {
  day: Day;
  detail: PlanDetail;
  stops: Stop[];
  kindLabels: Record<StopKind, string>;
  /** Pixels per minute — the scale the whole view is drawn at. */
  pxPerMin: number;
  selectedStopId: string | null;
  onSelectStop: (stopId: string | null) => void;
  renderStopActions: (stop: Stop) => React.ReactNode;
}

export function DayCanvas({
  day,
  detail,
  stops,
  kindLabels,
  pxPerMin,
  selectedStopId,
  onSelectStop,
  renderStopActions,
}: DayCanvasProps) {
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const span = Math.max(1, windowEnd - windowStart);
  const sky = daySky(day, detail, stops);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  // Where the plan actually ends, which may be past the window's close.
  const lastEnd = stops.reduce((acc, s) => Math.max(acc, hhmmToMin(s.plannedArrival) + s.durationMin), windowStart);
  const overrunMin = Math.max(0, lastEnd - windowEnd);
  // The canvas covers the window plus anything that spills past it, so an
  // impossible day is drawn overflowing rather than silently clipped.
  const canvasMin = span + overrunMin;
  const pct = (min: number) => ((min - windowStart) / canvasMin) * 100;

  const hours: number[] = [];
  for (let m = Math.ceil(windowStart / 60) * 60; m <= windowStart + canvasMin; m += 60) hours.push(m);

  return (
    <div className="daycanvas" style={{ height: `${Math.round(canvasMin * pxPerMin)}px` } as CSSProperties}>
      {sky && (
        <div
          className="dc-sky"
          aria-hidden
          style={{
            background: `linear-gradient(180deg, ${sky.stops})`,
            // The window's sky is painted across the window only; an overrun
            // hangs below it in the dark, which is the honest picture.
            height: `${(span / canvasMin) * 100}%`,
          }}
        />
      )}

      <div className="dc-hours" aria-hidden>
        {hours.map((m) => (
          <span key={m} className="dc-hour" style={{ top: `${pct(m)}%` }}>
            <i>{String(Math.floor(m / 60) % 24).padStart(2, '0')}:00</i>
          </span>
        ))}
      </div>

      {sky && sky.setAt > 0 && sky.setAt < 100 && (
        <div className="dc-sunset" style={{ top: `${pct(windowStart + (sky.setAt / 100) * span)}%` }}>
          <span>sunset {sky.set}</span>
        </div>
      )}

      {overrunMin > 0 && (
        <div className="dc-windowend" style={{ top: `${pct(windowEnd)}%` }}>
          <span>window closes {day.windowEnd}</span>
        </div>
      )}

      <div className="dc-track">
        {stops.map((stop, i) => {
          const start = hhmmToMin(stop.plannedArrival);
          const place = placeById.get(stop.placeId);
          const heightPx = stop.durationMin * pxPerMin;
          const legIn = detail.legs.find((l) => l.toStopId === stop.id);
          const prev = stops[i - 1];
          const gapStart = prev ? hhmmToMin(prev.plannedArrival) + prev.durationMin : windowStart;
          const slack = start - gapStart - (legIn?.durationMin ?? 0);
          const selected = selectedStopId === stop.id;
          // Judged on when the stop ENDS, not when it starts. The fixture note
          // for Day 6 — "the 14:45 Arashiyama arrival leaves little daylight
          // for the grove" — is about a visit that begins in the light and runs
          // out of it, which a start-time test would miss entirely.
          const afterSunset = !!sky && start + stop.durationMin - windowStart > (sky.setAt / 100) * span;

          const photo = heightPx >= SIZE_PHOTO ? place?.photoUrls?.[0] : undefined;

          return (
            <Fragment key={stop.id}>
              {/* Only legs *within* the day are drawn to scale. The leg into the
                  first stop starts on the previous day, so there is no honest
                  gap to size it against — squeezing a 25-minute train into the
                  15 minutes before the window opens would draw a lie. */}
              {legIn && i > 0 && start > gapStart && (
                <div className="dc-leg" style={{ top: `${pct(gapStart)}%`, height: `${pct(start) - pct(gapStart)}%` }}>
                  <span className={`leg-chip${legIn.feasibility !== 'ok' ? ` ${legIn.feasibility}` : ''}`}>
                    {MODE_ICON[legIn.mode]} {legIn.durationMin} min · {(legIn.distanceM / 1000).toFixed(1)} km
                    {legIn.feasibilityNote && ` — ${legIn.feasibilityNote}`}
                  </span>
                  {slack >= 15 && <span className="dc-slack">{formatDuration(slack)} spare</span>}
                </div>
              )}

              <article
                className={`dc-blk ${sizeClass(heightPx)}${selected ? ' sel' : ''}${
                  afterSunset ? ' after-dark' : ''
                }${photo ? ' has-photo' : ''}`}
                style={{ top: `${pct(start)}%`, height: `${(stop.durationMin / canvasMin) * 100}%` }}
                onClick={() => onSelectStop(selected ? null : stop.id)}
              >
                {/* A long stop earns its picture. Duration buys room, and room
                    that would otherwise be empty is the place you are spending
                    it in — which is also what stops a proportional timeline
                    from looking like a mostly-blank spreadsheet. */}
                {photo && <img className="dc-blk-photo" src={photo} alt="" loading="lazy" />}
                <div className="dc-blk-body">
                  <div className="dc-blk-head">
                    <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
                    <strong>{place?.name ?? stop.placeId}</strong>
                    {stop.booking && <span className="badge">booked</span>}
                  </div>
                  <div className="dc-blk-meta">
                    <span>
                      {stop.plannedArrival} · {formatDuration(stop.durationMin)}
                    </span>
                    {afterSunset && <span className="dc-dark-tag">after dark</span>}
                  </div>
                  {stop.notes && <p className="dc-blk-note">{stop.notes}</p>}
                  {selected && <div className="dc-blk-actions">{renderStopActions(stop)}</div>}
                </div>
              </article>
            </Fragment>
          );
        })}

        {/* Whatever is left of the window after the last stop — the slack you
            can actually see, rather than a percentage in a badge. */}
        {lastEnd < windowEnd && (
          <div className="dc-tail" style={{ top: `${pct(lastEnd)}%`, height: `${100 - pct(lastEnd)}%` }}>
            <span>{formatDuration(windowEnd - lastEnd)} unplanned</span>
          </div>
        )}
      </div>
    </div>
  );
}
