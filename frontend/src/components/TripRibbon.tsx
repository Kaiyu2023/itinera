import { useEffect, useRef } from 'react';
import { daySky, skyGradient } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { formatDuration } from './hooks';
import { KindGlyph } from './KindGlyph';
import { MoonGlyph, SunGlyph, WeatherGlyph, CONDITION_LABEL } from './SkyGlyph';
import type { TripWeather } from '../lib/weather';
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
 *
 * Each day is laid out on its own clock, not on elapsed engaged time. The
 * first cut packed stop against leg against stop with the gaps squeezed out,
 * which drew every day as starting at the same instant and made the one thing
 * the ribbon is for — comparing days — impossible. Now a day that opens at
 * 07:00 and one that opens at 14:00 are offset from each other, empty time is
 * empty, and the sky behind the line can be painted at the right place because
 * there is a real x-axis to hang it on.
 */

/** Horizontal scale. Small enough that a week is a couple of screens. */
const PX_PER_MIN = 0.3;
/** A stop narrower than this gets no label; the line still shows its extent. */
const LABEL_MIN_PX = 58;
/** …and narrower than this gets no glyph either. A 13px icon clipped into 10px
    of chip is not a smaller icon, it is a different and wrong icon — which is
    what the ribbon was shipping. */
const GLYPH_MIN_PX = 21;
/** A stretch of light or dark shorter than this gets no token: two hours of
    evening is worth a moon, twenty minutes is worth nothing. */
const SEGMENT_MIN_MIN = 55;

const MODE_CLASS: Record<string, string> = {
  walk: 'walk',
  transit: 'transit',
  drive: 'drive',
  flight: 'flight',
};

const hhmm = (min: number) =>
  `${String(Math.floor(min / 60) % 24).padStart(2, '0')}:${String(Math.round(min) % 60).padStart(2, '0')}`;

export function TripRibbon({
  days,
  detail,
  kindLabels,
  weather,
  active,
  onSelect,
}: {
  days: Day[];
  detail: PlanDetail;
  kindLabels: Record<StopKind, string>;
  weather: TripWeather;
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
          const sky = daySky(day, detail, stops);
          const wx = weather[day.id];

          const windowStart = hhmmToMin(day.windowStart);
          const windowEnd = hhmmToMin(day.windowEnd);
          const lastEnd = stops.reduce(
            (acc, s) => Math.max(acc, hhmmToMin(s.plannedArrival) + s.durationMin),
            windowEnd,
          );
          const spanMin = Math.max(60, lastEnd - windowStart);
          const at = (min: number) => ((min - windowStart) / spanMin) * 100;

          const parts: React.ReactNode[] = [];
          stops.forEach((stop, i) => {
            const start = hhmmToMin(stop.plannedArrival);
            const leg = detail.legs.find((l) => l.toStopId === stop.id);
            if (leg && i > 0) {
              const legStart = start - leg.durationMin;
              parts.push(
                <span
                  key={`${stop.id}-leg`}
                  /* `rb-warn`, not `warn`: the app already has a global `.warn`
                     rule for advisory boxes, and a 3px leg inheriting its 8px
                     padding and tinted fill drew an orange brick across the
                     line. */
                  className={`rb-leg ${MODE_CLASS[leg.mode] ?? 'transit'}${leg.feasibility !== 'ok' ? ' rb-warn' : ''}`}
                  style={{ left: `${at(legStart)}%`, width: `${Math.max(0.4, at(start) - at(legStart))}%` }}
                  title={`${leg.mode} ${leg.durationMin} min${leg.feasibilityNote ? ` — ${leg.feasibilityNote}` : ''}`}
                />,
              );
            }
            const w = stop.durationMin * PX_PER_MIN;
            const place = placeById.get(stop.placeId);
            parts.push(
              <span
                key={stop.id}
                className="rb-stop"
                style={{ left: `${at(start)}%`, width: `${at(start + stop.durationMin) - at(start)}%` }}
                title={`${place?.name ?? stop.placeId} · ${stop.plannedArrival} · ${formatDuration(stop.durationMin)}`}
              >
                {w >= GLYPH_MIN_PX && <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />}
                {w >= LABEL_MIN_PX && <i className="rb-name">{place?.name ?? stop.placeId}</i>}
              </span>,
            );
          });

          const start = stops[0]?.plannedArrival ?? day.windowStart;
          const end = hhmm(lastEnd);

          // A legend for the band, not a timestamp — the sun sits in the middle
          // of the lit stretch and the moon in the middle of the dark one. The
          // first cut pinned them to the horizons themselves, which is precise
          // and useless here: every day in the fixture opens after sunrise, so
          // no day ever got a sun. Whether a day is mostly light or mostly dark
          // is the question this row exists to answer at a glance. The exact
          // times are the day canvas's job, and the tooltip's.
          const bandEnd = windowStart + spanMin;
          const segments: Array<{ key: string; from: number; to: number }> = sky
            ? [
                { key: 'moon-dawn', from: windowStart, to: Math.min(bandEnd, sky.riseMin) },
                { key: 'sun', from: Math.max(windowStart, sky.riseMin), to: Math.min(bandEnd, sky.setMin) },
                { key: 'moon', from: Math.max(windowStart, sky.setMin), to: bandEnd },
              ].filter((s) => s.to - s.from >= SEGMENT_MIN_MIN)
            : [];

          return (
            <button
              key={day.id}
              type="button"
              data-day={day.id}
              className={`rb-day${day.id === active ? ' active' : ''}`}
              aria-pressed={day.id === active}
              onClick={() => onSelect(day.id)}
            >
              <span className="rb-daynum">
                {String(dayIndex + 1).padStart(2, '0')}
                <i>{day.cityHint}</i>
              </span>

              <span className="rb-sky" style={{ width: `${Math.round(spanMin * PX_PER_MIN)}px` }}>
                {sky && (
                  <span
                    className="rb-wash"
                    aria-hidden
                    style={{
                      background: `linear-gradient(90deg, ${skyGradient(sky.riseMin, sky.setMin, windowStart, spanMin)})`,
                    }}
                  />
                )}
                {sky && sky.setMin < windowStart + spanMin && (
                  <span
                    className="rb-stars"
                    aria-hidden
                    style={{ left: `${Math.max(0, at(sky.setMin))}%`, right: 0 }}
                  />
                )}
                {segments.map((seg) => (
                  <span
                    key={seg.key}
                    className={`rb-hz ${seg.key.startsWith('sun') ? 'sun' : 'moon'}`}
                    style={{ left: `${at((seg.from + seg.to) / 2)}%` }}
                    title={sky ? `daylight ${sky.rise}–${sky.set}` : undefined}
                  >
                    {seg.key.startsWith('sun') ? <SunGlyph /> : <MoonGlyph />}
                  </span>
                ))}
                {/* Both the stations and the track need one substrate to read
                    against: a grey leg drawn straight onto the sky vanishes at
                    dusk and again at dawn, twice per day. */}
                <span className="rb-rail" aria-hidden />
                <span className="rb-line">{parts}</span>
              </span>

              <span className="rb-foot">
                <span className="rb-clock">
                  {start}–{end}
                </span>
                {wx && (
                  <span
                    className={`rb-wx ${wx.source}`}
                    title={
                      wx.source === 'forecast'
                        ? `Forecast: ${CONDITION_LABEL[wx.condition]}, ${wx.tempMax}°/${wx.tempMin}°, ${wx.wetChance}% chance of rain`
                        : `Typical for this date (${wx.years?.[0]}–${wx.years?.[1]}): ${CONDITION_LABEL[wx.condition]}, ${wx.tempMax}°/${wx.tempMin}°, wet in ${wx.wetChance}% of those years`
                    }
                  >
                    <WeatherGlyph condition={wx.condition} label={CONDITION_LABEL[wx.condition]} />
                    {wx.tempMax}°
                  </span>
                )}
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
