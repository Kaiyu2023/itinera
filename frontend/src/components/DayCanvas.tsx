import { Fragment } from 'react';
import type { CSSProperties } from 'react';
import { daySky, skyGradient } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { formatDuration } from './hooks';
import { KindGlyph } from './KindGlyph';
import { MoonGlyph, SunGlyph } from './SkyGlyph';
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
 *
 * Two rules govern everything painted on the sky. Nothing may cross a block —
 * the horizon rules sit *behind* the column and put their labels in the gutter,
 * because a hairline through the middle of a stop's note is a bug wearing a
 * design's clothes. And anything printed straight onto the night band carries
 * its own surface, since the wash is darker than the page the text was legible
 * against.
 */

/** Pixel heights below which a block cannot hold a given tier of detail. */
const SIZE_FULL = 78; // name + meta + note
const SIZE_MEDIUM = 46; // name + meta
/** Above this a block has room to spare, so the place's photo fills it. */
const SIZE_PHOTO = 125;
/** Shorter gaps than this are not worth drawing as free time — five minutes of
    slack rendered as a dotted panel with a pill in it is noise wearing the
    costume of a feature. */
const GAP_MIN_MIN = 20;
/** An hour label this close to a horizon token would collide with it, so the
    sun or the moon takes that slot instead. */
const GUTTER_CLEARANCE_PX = 22;

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
  /** Raised from the unplanned tail, which is the one place on the screen
      where "there is room here" and "put something in it" are the same
      thought. */
  onAddStop?: () => void;
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
  onAddStop,
}: DayCanvasProps) {
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const span = Math.max(1, windowEnd - windowStart);
  const sky = daySky(day, detail, stops);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  // Where the plan actually ends, which may be past the window's close.
  const lastEnd = stops.reduce((acc, s) => Math.max(acc, hhmmToMin(s.plannedArrival) + s.durationMin), windowStart);
  const firstStart = stops.length ? hhmmToMin(stops[0].plannedArrival) : windowEnd;
  const overrunMin = Math.max(0, lastEnd - windowEnd);
  // The canvas covers the window plus anything that spills past it, so an
  // impossible day is drawn overflowing rather than silently clipped.
  const canvasMin = span + overrunMin;
  const canvasEnd = windowStart + canvasMin;
  const pct = (min: number) => ((min - windowStart) / canvasMin) * 100;

  /** Is this clock time inside the night band? */
  const isDark = (min: number) => !!sky && (min > sky.setMin || min < sky.riseMin);
  /** Far enough inside the canvas to be worth drawing a marker for. */
  const inView = (min: number) => min > windowStart + 6 && min < canvasEnd - 6;

  const horizons = sky
    ? (
        [
          { key: 'sunrise', min: sky.riseMin, time: sky.rise },
          { key: 'sunset', min: sky.setMin, time: sky.set },
        ] as const
      ).filter((h) => inView(h.min))
    : [];

  const hours: number[] = [];
  for (let m = Math.ceil(windowStart / 60) * 60; m <= canvasEnd; m += 60) hours.push(m);

  return (
    <div className="daycanvas" style={{ height: `${Math.round(canvasMin * pxPerMin)}px` } as CSSProperties}>
      {sky && (
        <div
          className="dc-sky"
          aria-hidden
          style={{
            background: `linear-gradient(180deg, ${skyGradient(sky.riseMin, sky.setMin, windowStart, canvasMin)})`,
          }}
        />
      )}

      {/* Stars, only where it is actually night. Cheap, and it does the job the
          "AFTER DARK" caption was doing on its own: you can see which end of
          the column the day ran into. */}
      {sky && sky.setMin < canvasEnd && (
        <div className="dc-stars" aria-hidden style={{ top: `${Math.max(0, pct(sky.setMin))}%`, bottom: 0 }} />
      )}
      {sky && sky.riseMin > windowStart && (
        <div className="dc-stars" aria-hidden style={{ top: 0, height: `${pct(sky.riseMin)}%` }} />
      )}

      <div className="dc-hours" aria-hidden>
        {hours.map((m) => {
          const taken = horizons.some((h) => Math.abs(h.min - m) * pxPerMin < GUTTER_CLEARANCE_PX);
          return (
            <span key={m} className="dc-hour" style={{ top: `${pct(m)}%` }}>
              {!taken && <i>{String(Math.floor(m / 60) % 24).padStart(2, '0')}:00</i>}
            </span>
          );
        })}
      </div>

      {/* Behind the column (z-index 1 against the track's 2), with the label in
          the gutter — so the rule can cross the whole day without ever landing
          on top of a stop. */}
      {horizons.map((h) => (
        <div key={h.key} className={`dc-horizon dc-${h.key}`} style={{ top: `${pct(h.min)}%` }}>
          <span className="dc-hz-mark">
            {h.key === 'sunrise' ? <SunGlyph label={`sunrise ${h.time}`} /> : <MoonGlyph label={`sunset ${h.time}`} />}
            <i>{h.time}</i>
          </span>
        </div>
      ))}

      {overrunMin > 0 && (
        <div className="dc-windowend" style={{ top: `${pct(windowEnd)}%` }}>
          <span>window closes {day.windowEnd}</span>
        </div>
      )}

      <div className="dc-track">
        {stops.map((stop, i) => {
          const start = hhmmToMin(stop.plannedArrival);
          const end = start + stop.durationMin;
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
          const afterSunset = isDark(end);
          // Where inside this block the sun actually goes down. A stop that
          // straddles the horizon darkens from that point rather than all over,
          // so the card shows you which part of the visit is in the light —
          // which is the entire question the Arashiyama note is asking.
          const duskAt =
            sky && sky.setMin > start && sky.setMin < end ? ((sky.setMin - start) / stop.durationMin) * 100 : 0;

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
                className={`dc-blk ${sizeClass(heightPx)} k-${stop.stopKind}${selected ? ' sel' : ''}${
                  afterSunset ? ' after-dark' : ''
                }${photo ? ' has-photo' : ''}`}
                style={
                  {
                    top: `${pct(start)}%`,
                    height: `${(stop.durationMin / canvasMin) * 100}%`,
                    '--dusk-at': `${duskAt.toFixed(1)}%`,
                  } as CSSProperties
                }
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
                    {afterSunset && (
                      <span className="dc-dark-tag">
                        <MoonGlyph />
                        after dark
                      </span>
                    )}
                  </div>
                  {stop.notes && <p className="dc-blk-note">{stop.notes}</p>}
                </div>
                {/* Deliberately a sibling of the body, not a child of it. Inside
                    the body it sat on the body's own scrim, so the glass had a
                    near-opaque surface to blur and rendered as a plain white
                    bar over the picture. Out here it floats on the photograph
                    and the blur has something to do. */}
                {selected && <div className="dc-blk-actions">{renderStopActions(stop)}</div>}
              </article>
            </Fragment>
          );
        })}

        {/* Whatever is left of the window after the last stop — the slack you
            can actually see, rather than a percentage in a badge. It is also
            the only place where "there is room here" and the button that fills
            it can be the same object, so the empty space is the affordance. */}
        {/* Free time at the head of the day is as unplanned as free time at the
            end of it — a window that opens at 08:30 for a first stop at 09:45
            has seventy-five minutes in it, and drawing only the trailing gap
            said otherwise. */}
        {firstStart - windowStart >= GAP_MIN_MIN && (
          <Gap from={windowStart} to={firstStart} lead pct={pct} pxPerMin={pxPerMin} onAddStop={onAddStop} />
        )}
        {windowEnd - lastEnd >= GAP_MIN_MIN && (
          <Gap from={lastEnd} to={windowEnd} pct={pct} pxPerMin={pxPerMin} onAddStop={onAddStop} />
        )}
      </div>
    </div>
  );
}

/**
 * Unplanned time, drawn as the space it is.
 *
 * It is a button wherever there is somewhere for the click to go, because this
 * is the one region of the screen where "there is room here" and the control
 * that fills it are the same object. The previous version — a 45° barber-pole
 * hatch with a grey caption — is the pattern browsers use for *disabled*, which
 * is close to the opposite of what free time in a plan means.
 */
function Gap({
  from,
  to,
  lead,
  pct,
  pxPerMin,
  onAddStop,
}: {
  from: number;
  to: number;
  lead?: boolean;
  pct: (min: number) => number;
  pxPerMin: number;
  onAddStop?: () => void;
}) {
  const style = { top: `${pct(from)}%`, height: `${pct(to) - pct(from)}%` };
  const cls = `dc-tail${lead ? ' lead' : ''}${(to - from) * pxPerMin < 62 ? ' sz-min' : ''}`;
  const label = (
    <span>
      <b>{formatDuration(to - from)} unplanned</b>
      {onAddStop && <em>＋ propose something here</em>}
    </span>
  );
  return onAddStop ? (
    <button type="button" className={cls} style={style} onClick={onAddStop}>
      {label}
    </button>
  ) : (
    <div className={cls} style={style}>
      {label}
    </div>
  );
}
