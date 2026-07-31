import type { CSSProperties } from 'react';
import { daySky, skyGradient } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { formatDuration } from './hooks';
import { KindGlyph } from './KindGlyph';
import { MoonGlyph, SunGlyph } from './SkyGlyph';
import { SkyScene } from './SkyScene';
import { MODE_ICON } from '../pages/planShared';
import type { Day, Leg, PlanDetail, Stop, StopKind } from '../api/types';

/**
 * A day drawn as a column of stops with the sky painted behind it, and a clock
 * down the left that says when each one happens.
 *
 * The point is to stop *describing* the day and start *depicting* it. In the
 * old timeline a four-hour temple visit and a five-minute walk were rows of
 * identical height, and the day was "87% full" because a badge said so.
 *
 * The first cut of this canvas answered that with a linear scale: one minute
 * was one fixed number of pixels, top to bottom, so a long visit was a long
 * block. That is Google Calendar's model, and it buys the duration read at a
 * price the rest of the card pays — a 25-minute stop got 47 pixels, which is
 * not enough room for a name, a time and a note, so its detail had to degrade
 * until it was a strip with a word in it. Half the column was a picture of how
 * long things took and the other half was unreadable.
 *
 * So the scale moved off the card and onto the axis. **Every stop is the same
 * height and every space between two stops is the same height** — enough room
 * for the place's photograph, its name, its time and its note, every time —
 * and the clock in the gutter absorbs the difference. The map from minutes to
 * pixels is now piecewise linear: constant *inside* each row, different from
 * row to row, since every row spends the same height on however many minutes
 * it happens to hold.
 *
 * So the hour marks are no longer evenly spaced, and that is the read the card
 * heights used to carry: where they bunch up, one thing is taking a long time;
 * where they spread out, the plan is turning over quickly.
 *
 * Two things survive from the proportional version because they were never
 * about height. A stop that runs past sunset darkens from the exact fraction
 * of its own height where the sun goes down (`--dusk-at`), which works because
 * the map is still linear *within* a card. And the horizon rules sit behind
 * the column with their labels out in the gutter, because a hairline through
 * the middle of a stop's note is a bug wearing a design's clothes.
 */

/** Shorter gaps than this are not worth labelling as free time — five minutes
    of slack announced as "5 min unplanned" is noise wearing the costume of a
    feature. The space is still drawn; only the label is withheld. */
const GAP_MIN_MIN = 20;
/** An hour label this close to a horizon token would collide with it, so the
    sun or the moon takes that slot instead. */
const GUTTER_CLEARANCE_PX = 22;
/** Two hour labels closer together than this are one smudge. Where they would
    collide the axis thins out, which is the honest reading of "this row is
    holding more minutes than it has pixels to print them in". */
const HOUR_MIN_PX = 26;
/** Half the height of an hour label. Nearer than this to an edge and half of
    it hangs outside the canvas, which is what put `14:00` above the top of its
    own rail. */
const LABEL_HALF_PX = 7;
/** Close enough to an edge that drawing a horizon token there is worse than
    leaving it off. */
const EDGE_PX = 14;

/** One band of the column: a stop, or the space between two of them. Rows are
    laid end to end in both dimensions — contiguous in time so the clock can be
    interpolated across them, contiguous in pixels so nothing overlaps. */
type Row =
  | { kind: 'stop'; from: number; to: number; top: number; height: number; stop: Stop; index: number }
  | { kind: 'gap'; from: number; to: number; top: number; height: number; edge: 'lead' | 'tail' | null };

/**
 * The column, as rows. Everything else on the canvas is positioned by asking
 * this layout where a given minute landed.
 */
function layoutDay(
  stops: Stop[],
  windowStart: number,
  windowEnd: number,
  cardHeight: number,
  gapHeight: number,
): { rows: Row[]; height: number } {
  const rows: Row[] = [];
  let y = 0;

  // A row may never start before the one above it ended. Two stops that
  // overlap are bad data, and the axis has to stay monotonic through them
  // rather than fold back on itself and put 11:00 above 10:00.
  const startOf = (from: number) => (rows.length ? Math.max(from, rows[rows.length - 1].to) : from);

  const addGap = (from: number, to: number, edge: 'lead' | 'tail' | null, height = gapHeight) => {
    const f = startOf(from);
    rows.push({ kind: 'gap', from: f, to: Math.max(f, to), top: y, height, edge });
    y += height;
  };

  if (!stops.length) {
    // A day with nothing in it is one wide invitation, not a hairline.
    addGap(windowStart, windowEnd, 'lead', cardHeight);
    return { rows, height: y };
  }

  let prevEnd = windowStart;
  stops.forEach((stop, index) => {
    const start = hhmmToMin(stop.plannedArrival);
    const end = start + stop.durationMin;
    // Constant distance between cards. The one exception is the top of the
    // column: a window that opens exactly on its first stop has no space to
    // give, and inventing some would draw free time that does not exist.
    if (index > 0 || start > windowStart) addGap(prevEnd, start, index === 0 ? 'lead' : null);
    const f = startOf(start);
    rows.push({ kind: 'stop', from: f, to: Math.max(f, end), top: y, height: cardHeight, stop, index });
    y += cardHeight;
    prevEnd = Math.max(prevEnd, end);
  });
  // Only when there is window left. A day that overruns its close ends on its
  // last stop, and the dotted rule says where the window shut.
  if (windowEnd > prevEnd) addGap(prevEnd, windowEnd, 'tail');

  return { rows, height: y };
}

export interface DayCanvasProps {
  day: Day;
  detail: PlanDetail;
  stops: Stop[];
  kindLabels: Record<StopKind, string>;
  /** Every stop is drawn this tall, whatever the clock says. */
  cardHeight: number;
  /** And every space between two stops is drawn this tall. */
  gapHeight: number;
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
  cardHeight,
  gapHeight,
  selectedStopId,
  onSelectStop,
  renderStopActions,
  onAddStop,
}: DayCanvasProps) {
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const sky = daySky(day, detail, stops);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  const { rows, height } = layoutDay(stops, windowStart, windowEnd, cardHeight, gapHeight);
  const axisStart = rows[0].from;
  const axisEnd = rows[rows.length - 1].to;

  /**
   * The clock, as geometry. Linear inside a row, and the rows are different
   * scales — which is what "no longer divided evenly by hours" means in one
   * function. Clamped at both ends, so a sunrise before the window opens
   * resolves to the top of the canvas rather than to a negative offset.
   */
  const yOf = (min: number) => {
    if (min <= axisStart) return 0;
    for (const row of rows) {
      if (min <= row.to) {
        const span = row.to - row.from;
        return span > 0 ? row.top + ((min - row.from) / span) * row.height : row.top;
      }
    }
    return height;
  };
  const fracOf = (min: number) => yOf(min) / Math.max(1, height);

  /** Is this clock time inside the night band? */
  const isDark = (min: number) => !!sky && (min > sky.setMin || min < sky.riseMin);

  const horizons = sky
    ? (
        [
          { key: 'sunrise', min: sky.riseMin, time: sky.rise },
          { key: 'sunset', min: sky.setMin, time: sky.set },
        ] as const
      ).filter((h) => yOf(h.min) > EDGE_PX && yOf(h.min) < height - EDGE_PX)
    : [];

  const hours: Array<{ min: number; y: number; label: boolean }> = [];
  let lastHourY = -Infinity;
  for (let m = Math.ceil(axisStart / 60) * 60; m <= axisEnd; m += 60) {
    const y = yOf(m);
    if (y - lastHourY < HOUR_MIN_PX) continue;
    lastHourY = y;
    hours.push({ min: m, y, label: !horizons.some((h) => Math.abs(yOf(h.min) - y) < GUTTER_CLEARANCE_PX) });
  }

  return (
    <div className="daycanvas" style={{ height: `${Math.round(height)}px` } as CSSProperties}>
      {sky && (
        <div
          className="dc-sky"
          aria-hidden
          style={{
            background: `linear-gradient(180deg, ${skyGradient(
              sky.riseMin,
              sky.setMin,
              axisStart,
              axisEnd - axisStart,
              (min) => fracOf(min) * 100,
            )})`,
          }}
        />
      )}

      {/* The same scene the ribbon paints, turned on its side and dialled down:
          here it is atmosphere behind a column of cards, not the primary read.
          It still does the job the "AFTER DARK" caption was doing alone — you
          can see which end of the column the day ran into. */}
      {sky && (
        <div className="dc-scene" aria-hidden>
          <SkyScene
            axis="y"
            seed={day.id}
            rise={fracOf(sky.riseMin)}
            set={fracOf(sky.setMin)}
            noon={fracOf((sky.riseMin + sky.setMin) / 2)}
            density={Math.round(height * 0.07)}
            cross={[0.08, 0.94]}
            bodySize={48}
            cloudSize={[46, 96]}
            starSize={[8, 20]}
          />
        </div>
      )}

      {/* The clock's substrate. The sky runs under the gutter now, so the hours
          need something to be printed on that is the same colour at 07:00 and
          at 21:00. */}
      <span className="dc-rail glass" aria-hidden />

      <div className="dc-hours" aria-hidden>
        {hours.map(({ min, y, label }) => (
          <span
            key={min}
            className={`dc-hour${y < LABEL_HALF_PX ? ' at-top' : ''}${y > height - LABEL_HALF_PX ? ' at-end' : ''}`}
            style={{ top: `${y}px` }}
          >
            {label && <i>{String(Math.floor(min / 60) % 24).padStart(2, '0')}:00</i>}
          </span>
        ))}
      </div>

      {/* Behind the column (z-index 1 against the track's 2), with the label in
          the gutter — so the rule can cross the whole day without ever landing
          on top of a stop. */}
      {horizons.map((h) => (
        <div key={h.key} className={`dc-horizon dc-${h.key}`} style={{ top: `${yOf(h.min)}px` }}>
          <span className="dc-hz-mark">
            {h.key === 'sunrise' ? <SunGlyph label={`sunrise ${h.time}`} /> : <MoonGlyph label={`sunset ${h.time}`} />}
            <i>{h.time}</i>
          </span>
        </div>
      ))}

      {axisEnd > windowEnd && (
        <div className="dc-windowend" style={{ top: `${yOf(windowEnd)}px` }}>
          <span>window closes {day.windowEnd}</span>
        </div>
      )}

      <div className="dc-track">
        {rows.map((row, i) => {
          const style = { top: `${row.top}px`, height: `${row.height}px` };

          if (row.kind === 'gap') {
            const next = rows[i + 1];
            // Only legs *within* the day are drawn. The leg into the first stop
            // starts on the previous day, so there is no honest gap to hang it
            // in — a 25-minute train squeezed into the space before the window
            // opens would draw a lie.
            const legIn: Leg | undefined =
              row.edge !== 'lead' && next?.kind === 'stop'
                ? detail.legs.find((l) => l.toStopId === next.stop.id)
                : undefined;

            if (legIn) {
              const slack = row.to - row.from - legIn.durationMin;
              return (
                <div key={`gap-${row.top}`} className="dc-leg" style={style}>
                  <span className={`leg-chip${legIn.feasibility !== 'ok' ? ` ${legIn.feasibility}` : ''}`}>
                    {MODE_ICON[legIn.mode]} {legIn.durationMin} min · {(legIn.distanceM / 1000).toFixed(1)} km
                    {legIn.feasibilityNote && ` — ${legIn.feasibilityNote}`}
                  </span>
                  {slack >= 15 && <span className="dc-slack">{formatDuration(slack)} spare</span>}
                </div>
              );
            }

            // Unplanned time, wherever it is: at the head of the day, at its
            // end, or between two stops with nothing travelling between them.
            // Drawing only the trailing gap said a window that opens at 08:30
            // for a first stop at 09:45 had those seventy-five minutes spoken
            // for.
            if (row.to - row.from < GAP_MIN_MIN) return null;
            return (
              <Gap
                key={`gap-${row.top}`}
                from={row.from}
                to={row.to}
                lead={row.edge === 'lead'}
                style={style}
                onAddStop={onAddStop}
              />
            );
          }

          const { stop } = row;
          const start = row.from;
          const end = row.to;
          const place = placeById.get(stop.placeId);
          const selected = selectedStopId === stop.id;
          // Judged on when the stop ENDS, not when it starts. The fixture note
          // for Day 6 — "the 14:45 Arashiyama arrival leaves little daylight
          // for the grove" — is about a visit that begins in the light and runs
          // out of it, which a start-time test would miss entirely.
          const afterSunset = isDark(end);
          // Where inside this block the sun actually goes down. A stop that
          // straddles the horizon darkens from that point rather than all over,
          // so the card shows you which part of the visit is in the light —
          // which is the entire question the Arashiyama note is asking. Still
          // exact under a piecewise axis: the map is linear inside a card.
          const duskAt =
            sky && sky.setMin > start && sky.setMin < end ? ((sky.setMin - start) / (end - start)) * 100 : 0;
          // Every card is now tall enough for the place's photograph, so the
          // only question left is whether there is one.
          const photo = place?.photoUrls?.[0];

          return (
            <article
              key={stop.id}
              className={`dc-blk k-${stop.stopKind}${selected ? ' sel' : ''}${afterSunset ? ' after-dark' : ''}${
                photo ? ' has-photo' : ''
              }`}
              style={{ ...style, '--dusk-at': `${duskAt.toFixed(1)}%` } as CSSProperties}
              onClick={() => onSelectStop(selected ? null : stop.id)}
            >
              {/* The picture fills the card corner to corner — see `.dc-blk`,
                  whose frame is an inset shadow precisely so that a photograph
                  covers it rather than stopping short and leaving the card's
                  own edge showing as a border around the image. */}
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
          );
        })}
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
  style,
  onAddStop,
}: {
  from: number;
  to: number;
  lead?: boolean;
  style: CSSProperties;
  onAddStop?: () => void;
}) {
  const cls = `dc-tail${lead ? ' lead' : ''}`;
  const label = (
    /* The region itself draws nothing now — the sky shows through it, which is
       the honest picture of "nothing is happening here". That leaves this pill
       floating on a photograph of the weather, so it has to be glass. */
    <span className="glass">
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
