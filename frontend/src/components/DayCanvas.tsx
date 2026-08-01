import type { CSSProperties } from 'react';
import { daySky, skyGradient } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { useI18n } from '../i18n';
import { formatPlanDistance, formatPlanDuration } from '../i18n/messages.plan';
import { KindGlyph } from './KindGlyph';
import { ModeGlyph } from './ModeGlyph';
import { MoonGlyph, SunGlyph } from './SkyGlyph';
import { SkyScene } from './SkyScene';
import type { Day, Leg, PlanDetail, Stop, StopKind } from '../api/types';

/** Shorter empty regions are real, but not useful enough to become controls. */
const GAP_MIN_MIN = 20;
const LABEL_HALF_PX = 7;
const MODE_KEY = {
  walk: 'plan.mode.walk',
  transit: 'plan.mode.transit',
  drive: 'plan.mode.drive',
  flight: 'plan.mode.flight',
} as const;

type Row =
  | { kind: 'stop'; from: number; to: number; stop: Stop; index: number }
  | { kind: 'gap'; from: number; to: number; edge: 'lead' | 'tail' | null };

/**
 * Build a chronological model without assigning presentation height. A minute
 * receives the same number of pixels everywhere; this is the contract that
 * lets the gutter behave like a clock rather than an illustration.
 */
function layoutDay(stops: Stop[], windowStart: number, windowEnd: number): Row[] {
  const rows: Row[] = [];

  if (!stops.length) return [{ kind: 'gap', from: windowStart, to: windowEnd, edge: 'lead' }];

  let previousEnd = windowStart;
  stops.forEach((stop, index) => {
    const start = hhmmToMin(stop.plannedArrival);
    const end = start + stop.durationMin;
    if (index === 0 && start > windowStart) rows.push({ kind: 'gap', from: windowStart, to: start, edge: 'lead' });
    if (index > 0) {
      // Keep an arrival boundary even when there is no positive gap. Its leg
      // and feasibility warning still matter; the label may straddle the line
      // without changing the time geometry.
      rows.push({ kind: 'gap', from: start > previousEnd ? previousEnd : start, to: start, edge: null });
    }
    rows.push({ kind: 'stop', from: start, to: end, stop, index });
    previousEnd = Math.max(previousEnd, end);
  });

  if (windowEnd > previousEnd) rows.push({ kind: 'gap', from: previousEnd, to: windowEnd, edge: 'tail' });
  return rows;
}

export interface DayCanvasProps {
  day: Day;
  detail: PlanDetail;
  stops: Stop[];
  kindLabels: Record<StopKind, string>;
  /** The shared linear scale for the entire canvas. */
  hourHeight: number;
  selectedStopId: string | null;
  onSelectStop: (stopId: string | null) => void;
  onAddStop?: (initialSlot: string) => void;
}

export function DayCanvas({
  day,
  detail,
  stops,
  kindLabels,
  hourHeight,
  selectedStopId,
  onSelectStop,
  onAddStop,
}: DayCanvasProps) {
  const { locale, t } = useI18n();
  const duration = (minutes: number) => formatPlanDuration(minutes, t);
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const rows = layoutDay(stops, windowStart, windowEnd);
  const stopStarts = stops.map((stop) => hhmmToMin(stop.plannedArrival));
  const stopEnds = stops.map((stop, index) => stopStarts[index] + stop.durationMin);
  const axisStart = Math.min(windowStart, ...stopStarts);
  const axisEnd = Math.max(windowEnd, ...stopEnds);
  const pxPerMin = hourHeight / 60;
  const height = Math.max(hourHeight, (axisEnd - axisStart) * pxPerMin);
  const yOf = (min: number) => (min - axisStart) * pxPerMin;
  const sky = daySky(day, detail, stops);
  const placeById = new Map(detail.places.map((place) => [place.id, place]));

  const horizons = sky
    ? (
        [
          { key: 'sunrise', min: sky.riseMin, time: sky.rise },
          { key: 'sunset', min: sky.setMin, time: sky.set },
        ] as const
      ).filter((horizon) => horizon.min >= axisStart && horizon.min <= axisEnd)
    : [];

  const hours: number[] = [];
  for (let min = Math.ceil(axisStart / 60) * 60; min <= axisEnd; min += 60) hours.push(min);

  // The trip ribbon and the compact daylight strip already paint time with the
  // shared solar ramp. The clock rail is the vertical form of that same
  // language: the geometry is calculated against the *actual* canvas extent,
  // including a stop that runs beyond the planning window.
  const railSky = sky
    ? `linear-gradient(180deg, ${skyGradient(sky.riseMin, sky.setMin, axisStart, axisEnd - axisStart)})`
    : undefined;
  const railFrac = (min: number) => (min - axisStart) / Math.max(1, axisEnd - axisStart);

  return (
    <div
      className="daycanvas"
      style={{ height: `${Math.round(height)}px`, '--dc-hour-height': `${hourHeight}px` } as CSSProperties}
    >
      <span
        className={`dc-rail${railSky ? ' has-sky' : ''}`}
        style={railSky ? ({ '--dc-rail-sky': railSky } as CSSProperties) : undefined}
        aria-hidden
      >
        {sky && (
          <SkyScene
            axis="y"
            seed={`${day.id}-rail`}
            rise={railFrac(sky.riseMin)}
            set={railFrac(sky.setMin)}
            noon={railFrac((sky.riseMin + sky.setMin) / 2)}
            density={Math.max(7, Math.min(16, Math.round(height / 86)))}
            cross={[0.18, 0.88]}
            bodySize={14}
            bodyCross={0.76}
            cloudSize={[10, 17]}
            starSize={[3.5, 6.5]}
          />
        )}
      </span>

      <div className="dc-hours" aria-hidden>
        {hours.map((min) => {
          const y = yOf(min);
          return (
            <span
              key={min}
              className={`dc-hour${y < LABEL_HALF_PX ? ' at-top' : ''}${y > height - LABEL_HALF_PX ? ' at-end' : ''}`}
              style={{ top: `${y}px` }}
            >
              <i>{String(Math.floor(min / 60) % 24).padStart(2, '0')}:00</i>
            </span>
          );
        })}
      </div>

      {horizons.map((horizon) => (
        <div key={horizon.key} className={`dc-horizon dc-${horizon.key}`} style={{ top: `${yOf(horizon.min)}px` }}>
          <span className="dc-hz-mark">
            {horizon.key === 'sunrise' ? (
              <SunGlyph label={`${t('plan.day.sunrise')} ${horizon.time}`} />
            ) : (
              <MoonGlyph label={`${t('plan.day.sunset')} ${horizon.time}`} />
            )}
            <i>{horizon.time}</i>
          </span>
        </div>
      ))}

      {axisEnd > windowEnd && (
        <div className="dc-windowend" style={{ top: `${yOf(windowEnd)}px` }}>
          <span>{t('plan.day.windowCloses', { time: day.windowEnd })}</span>
        </div>
      )}

      <div className="dc-track">
        {rows.map((row, index) => {
          const top = yOf(row.from);
          const rowHeight = Math.max(1, yOf(row.to) - top);
          const style = { top: `${top}px`, height: `${rowHeight}px` };

          if (row.kind === 'gap') {
            const next = rows[index + 1];
            const legIn: Leg | undefined =
              row.edge !== 'lead' && next?.kind === 'stop'
                ? detail.legs.find((leg) => leg.toStopId === next.stop.id)
                : undefined;

            if (legIn && next?.kind === 'stop') {
              const slack = Math.max(0, row.to - row.from - legIn.durationMin);
              const destination = placeById.get(next.stop.placeId)?.name ?? t('plan.stop.next');
              const distance = formatPlanDistance(legIn.distanceM, locale, t);
              const mode = t(MODE_KEY[legIn.mode]);
              const legLabel = t('plan.leg.to', { mode, destination });
              return (
                <div
                  key={`gap-${row.from}-${row.to}`}
                  className={`dc-leg${legIn.feasibility !== 'ok' ? ` ${legIn.feasibility}` : ''}`}
                  style={style}
                  aria-label={`${legLabel}, ${duration(legIn.durationMin)}, ${distance}${
                    slack >= 15 ? `, ${t('plan.day.buffer', { duration: duration(slack) })}` : ''
                  }`}
                >
                  <span className="dc-leg-line">
                    <ModeGlyph mode={legIn.mode} label={mode} />
                    <strong>{legLabel}</strong>
                    <span className="dc-leg-duration">{duration(legIn.durationMin)}</span>
                    <span className="dc-leg-distance">{distance}</span>
                    <span className="dc-leg-arrival">{t('plan.day.arrive', { time: next.stop.plannedArrival })}</span>
                    {slack >= 15 && (
                      <span className="dc-leg-buffer">{t('plan.day.buffer', { duration: duration(slack) })}</span>
                    )}
                    {legIn.feasibilityNote && <em title={legIn.feasibilityNote}>{legIn.feasibilityNote}</em>}
                  </span>
                </div>
              );
            }

            if (row.to - row.from < GAP_MIN_MIN) return null;
            const previous = rows[index - 1];
            const initialSlot = previous?.kind === 'stop' ? previous.stop.id : 'first';
            return (
              <Gap
                key={`gap-${row.from}-${row.to}`}
                from={row.from}
                to={row.to}
                lead={row.edge === 'lead'}
                style={style}
                onAddStop={onAddStop ? () => onAddStop(initialSlot) : undefined}
              />
            );
          }

          const { stop } = row;
          const place = placeById.get(stop.placeId);
          const selected = selectedStopId === stop.id;
          const crossesSunset = !!sky && row.from < sky.setMin && row.to > sky.setMin;
          const afterDark = !!sky && row.from >= sky.setMin;
          const density = rowHeight >= 90 ? 'roomy' : rowHeight >= 50 ? 'compact' : 'micro';
          const featured = rowHeight >= 180;
          const photo = density === 'micro' ? undefined : place?.photoUrls?.[0];
          const guideSummary = place?.guide?.summary?.trim();

          return (
            <article
              key={stop.id}
              className={`dc-blk k-${stop.stopKind} ${density}${featured ? ' feature' : ''}${selected ? ' sel' : ''}${afterDark || crossesSunset ? ' after-dark' : ''}${
                photo ? ' has-photo' : ''
              }`}
              style={style}
            >
              <button
                type="button"
                id={`timeline-stop-${stop.id}`}
                className="dc-blk-hit"
                aria-pressed={selected}
                aria-expanded={selected}
                aria-controls={selected ? `timeline-stop-details-${stop.id}` : undefined}
                aria-label={`${kindLabels[stop.stopKind]} ${place?.name ?? stop.placeId}, ${stop.plannedArrival}, ${duration(
                  stop.durationMin,
                )}`}
                onClick={() => onSelectStop(selected ? null : stop.id)}
              >
                <span className="dc-blk-body">
                  <span className="dc-blk-head">
                    <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />
                    <strong>{place?.name ?? stop.placeId}</strong>
                    {stop.booking && <span className="badge">{t('plan.stop.booked')}</span>}
                  </span>
                  <span className="dc-blk-meta">
                    <span>
                      {stop.plannedArrival} · {duration(stop.durationMin)}
                    </span>
                    {(afterDark || crossesSunset) && (
                      <span className="dc-dark-tag">
                        <MoonGlyph />
                        {crossesSunset ? t('plan.day.sunsetTime', { time: sky?.set ?? '' }) : t('plan.day.afterDark')}
                      </span>
                    )}
                  </span>
                  {guideSummary && <span className="dc-blk-summary">{guideSummary}</span>}
                  {featured && !!place?.guide?.activityIdeas.length && (
                    <span className="dc-blk-ideas">
                      <span className="dc-blk-ideas-label">{t('plan.guide.ideas')}</span>
                      <span className="dc-blk-idea-list">
                        {place.guide.activityIdeas.slice(0, 3).map((idea) => (
                          <span key={idea.title}>{idea.title}</span>
                        ))}
                      </span>
                    </span>
                  )}
                  {featured && stop.notes && (
                    <span className="dc-blk-trip-note">
                      <b>{t('plan.stop.tripNote')}</b>
                      <span>{stop.notes}</span>
                    </span>
                  )}
                  {!featured && !guideSummary && stop.notes && (
                    <span className="dc-blk-trip-note">
                      <b>{t('plan.stop.tripNote')}</b>
                      <span>{stop.notes}</span>
                    </span>
                  )}
                </span>
                {photo && <img className="dc-blk-photo" src={photo} alt="" loading="lazy" />}
              </button>
            </article>
          );
        })}
      </div>
    </div>
  );
}

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
  const { t } = useI18n();
  const duration = formatPlanDuration(to - from, t);
  const className = `dc-tail${lead ? ' lead' : ''}`;
  const label = (
    <span>
      <b>{t('plan.day.free', { duration })}</b>
      {onAddStop && <em>{t('plan.day.addAStop')}</em>}
    </span>
  );

  return onAddStop ? (
    <button type="button" className={className} style={style} onClick={onAddStop}>
      {label}
    </button>
  ) : (
    <div className={className} style={style}>
      {label}
    </div>
  );
}
