import { useCallback, useEffect, useRef, useState } from 'react';
import { daySky, skyGradient } from '../lib/daylight';
import { hhmmToMin } from '../lib/sun';
import { useI18n } from '../i18n';
import { formatPlanDuration } from '../i18n/messages.plan';
import { KindGlyph } from './KindGlyph';
import { WeatherGlyph } from './SkyGlyph';
import { SkyScene } from './SkyScene';
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
/**
 * How much band a label plate needs before it can hold everything. Below the
 * first threshold the clock goes, below the second the weather goes too; the
 * band keeps its true width either way, because stretching a short day to fit
 * its own caption is the label deciding how long the day was.
 *
 * The clock is what gets dropped first, and not because it is less useful — the
 * band's extent *is* the clock, drawn to scale, so it is the one fact on the
 * plate that is already being said twice. The weather is said nowhere else.
 */
const PLATE_FULL_PX = 168;
const PLATE_WX_PX = 112;
/** A stop narrower than this gets no label; the line still shows its extent. */
const LABEL_MIN_PX = 58;
/** …and narrower than this gets no glyph either. A 13px icon clipped into 10px
    of chip is not a smaller icon, it is a different and wrong icon — which is
    what the ribbon was shipping. */
const GLYPH_MIN_PX = 21;

const MODE_CLASS: Record<string, string> = {
  walk: 'walk',
  transit: 'transit',
  drive: 'drive',
  flight: 'flight',
};

const MODE_KEY = {
  walk: 'plan.mode.walk',
  transit: 'plan.mode.transit',
  drive: 'plan.mode.drive',
  flight: 'plan.mode.flight',
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

const FEASIBILITY_KEY = {
  ok: 'plan.feasibility.ok',
  tight: 'plan.feasibility.tight',
  unreasonable: 'plan.feasibility.unreasonable',
  impossible: 'plan.feasibility.impossible',
} as const;

const hhmm = (min: number) =>
  `${String(Math.floor(min / 60) % 24).padStart(2, '0')}:${String(Math.round(min) % 60).padStart(2, '0')}`;

/** Pointer travel past which a press is a pan and not a click on a day. */
const DRAG_SLOP_PX = 4;

/**
 * Panning the ribbon, without a scrollbar.
 *
 * The trough was the third control on this axis — the day chips below it index
 * the same seven days, and clicking a day is the same navigation again — and it
 * was chrome describing something the picture already says. What replaces it:
 * the mask fade says *there is more*, the chevrons say *here is how to get it*
 * and are the only part that has to be discoverable, and drag-to-pan is what
 * everybody tries first anyway.
 *
 * The wheel is deliberately left alone. Hijacking `deltaY` over a full-width
 * band you must scroll past to reach the day below it trades a rare need for a
 * constant annoyance; shift+wheel and trackpad gestures already work natively.
 */
function usePan(ref: React.RefObject<HTMLDivElement | null>, dependency: number) {
  const [edges, setEdges] = useState({ back: false, fwd: false });
  const drag = useRef<{ x: number; from: number; moved: boolean } | null>(null);
  const swallowClick = useRef(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const back = el.scrollLeft > 2;
      const fwd = el.scrollLeft + el.clientWidth < el.scrollWidth - 2;
      // The fades are the same two facts as the chevrons, so they are measured
      // in the same place — but written to the DOM rather than rendered, because
      // this fires on every scroll frame and `dragging` is set imperatively on
      // the same element. A rendered className would wipe it mid-drag.
      el.classList.toggle('more-back', back);
      el.classList.toggle('more-fwd', fwd);
      setEdges({ back, fwd });
    };
    measure();
    el.addEventListener('scroll', measure, { passive: true });
    // The band's width is a function of the viewport, so "is there more" changes
    // on resize without anything scrolling.
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => {
      el.removeEventListener('scroll', measure);
      ro.disconnect();
    };
  }, [ref, dependency]);

  const page = useCallback(
    (dir: 1 | -1) => {
      const el = ref.current;
      if (!el) return;
      el.scrollBy({ left: dir * el.clientWidth * 0.8, behavior: 'smooth' });
    },
    [ref],
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    // Touch already pans this natively, and better — taking the pointer would
    // only cost the fling.
    if (e.pointerType === 'touch' || !ref.current) return;
    drag.current = { x: e.clientX, from: ref.current.scrollLeft, moved: false };
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    const el = ref.current;
    if (!d || !el) return;
    const dx = e.clientX - d.x;
    if (!d.moved && Math.abs(dx) < DRAG_SLOP_PX) return;
    if (!d.moved) {
      d.moved = true;
      el.classList.add('dragging');
      el.setPointerCapture(e.pointerId);
    }
    el.scrollLeft = d.from - dx;
  };

  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    const el = ref.current;
    drag.current = null;
    if (!d || !el) return;
    if (d.moved) {
      el.classList.remove('dragging');
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      // The click that follows this pointerup belongs to the drag, not to
      // whichever day happened to be under the cursor when it stopped.
      swallowClick.current = true;
    }
  };

  const onClickCapture = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!swallowClick.current) return;
    swallowClick.current = false;
    e.stopPropagation();
    e.preventDefault();
  };

  return {
    edges,
    page,
    trackProps: { onPointerDown, onPointerMove, onPointerUp: endDrag, onPointerCancel: endDrag, onClickCapture },
  };
}

function Chevron({ back }: { back?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.6} strokeLinecap="round" aria-hidden>
      <path d={back ? 'M15 5l-7 7 7 7' : 'M9 5l7 7-7 7'} />
    </svg>
  );
}

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
  const { t } = useI18n();
  const duration = (minutes: number) => formatPlanDuration(minutes, t);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const { edges, page, trackProps } = usePan(trackRef, days.length);
  const placeById = new Map(detail.places.map((p) => [p.id, p]));

  // Keep the selected day in view when it changes from elsewhere (the scrubber,
  // a deep link) — otherwise the ribbon silently disagrees with the canvas.
  useEffect(() => {
    const el = trackRef.current?.querySelector<HTMLElement>(`[data-day="${active}"]`);
    el?.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' });
  }, [active]);

  return (
    <section className="ribbon" aria-label={t('plan.ribbon.label')}>
      <button
        type="button"
        className="rb-page back glass"
        aria-label={t('plan.ribbon.earlier')}
        disabled={!edges.back}
        onClick={() => page(-1)}
      >
        <Chevron back />
      </button>
      <button
        type="button"
        className="rb-page fwd glass"
        aria-label={t('plan.ribbon.later')}
        disabled={!edges.fwd}
        onClick={() => page(1)}
      >
        <Chevron />
      </button>
      <div className="rb-track" ref={trackRef} {...trackProps}>
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
          // Fractions of the band, for the scene. Deliberately unclamped: a
          // window that opens after sunrise has a negative `rise`, and the
          // scene needs to know that rather than be told the sun came up at the
          // left edge — otherwise the stars creep into the morning.
          const frac = (min: number) => (min - windowStart) / spanMin;

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
                  title={`${t(MODE_KEY[leg.mode])} ${duration(leg.durationMin)}${
                    leg.feasibilityNote ? ` — ${leg.feasibilityNote}` : ''
                  }`}
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
                title={`${place?.name ?? stop.placeId} · ${stop.plannedArrival} · ${duration(stop.durationMin)}`}
              >
                {w >= GLYPH_MIN_PX && <KindGlyph kind={stop.stopKind} label={kindLabels[stop.stopKind]} />}
                {w >= LABEL_MIN_PX && <i className="rb-name">{place?.name ?? stop.placeId}</i>}
              </span>,
            );
          });

          const start = stops[0]?.plannedArrival ?? day.windowStart;
          const end = hhmm(lastEnd);
          const bandPx = Math.round(spanMin * PX_PER_MIN);
          const showClock = bandPx >= PLATE_FULL_PX;
          const showWx = bandPx >= PLATE_WX_PX;

          return (
            <button
              key={day.id}
              type="button"
              data-day={day.id}
              className={`rb-day${day.id === active ? ' active' : ''}`}
              aria-pressed={day.id === active}
              onClick={() => onSelect(day.id)}
              /* The plate drops the clock and the weather on a short band, so
                 hover has to be able to give them back. */
              title={t('plan.ribbon.dayTitle', { day: dayIndex + 1, city: day.cityHint, start, end })}
            >
              <span className="rb-sky" style={{ width: `${bandPx}px` }}>
                {sky && (
                  <span
                    className="rb-wash"
                    aria-hidden
                    style={{
                      background: `linear-gradient(90deg, ${skyGradient(sky.riseMin, sky.setMin, windowStart, spanMin)})`,
                    }}
                  />
                )}
                {sky && (
                  <SkyScene
                    axis="x"
                    seed={day.id}
                    rise={frac(sky.riseMin)}
                    set={frac(sky.setMin)}
                    noon={frac((sky.riseMin + sky.setMin) / 2)}
                    density={Math.round(spanMin * PX_PER_MIN * 0.22)}
                    /* The band is tall enough now to have a middle. Stars run
                       up behind the label plate on purpose — it is glass, and
                       glass with nothing behind it is just a lighter rectangle. */
                    cross={[0.13, 0.72]}
                    bodySize={24}
                    cloudSize={[20, 38]}
                    starSize={[6, 14]}
                  />
                )}
                {/* Both the stations and the track need one substrate to read
                    against: a grey leg drawn straight onto the sky vanishes at
                    dusk and again at dawn, twice per day. */}
                <span className="rb-rail" aria-hidden />
                <span className="rb-line">{parts}</span>

                {/* Everything the day has to say, on one pane over its own sky.
                    It used to be two rows of bare text stacked above and below
                    the band, which spent twice the height and left the picture
                    with a caption rather than a label. */}
                <span className="rb-plate glass">
                  <b className="rb-dnum">{String(dayIndex + 1).padStart(2, '0')}</b>
                  <i className="rb-city">{day.cityHint}</i>
                  {showClock && (
                    <span className="rb-clock">
                      {start}–{end}
                    </span>
                  )}
                  {showWx && wx && (
                    <span
                      className={`rb-wx ${wx.source}`}
                      title={
                        wx.source === 'forecast'
                          ? t('plan.weather.ribbonForecast', {
                              condition: t(WEATHER_KEY[wx.condition]),
                              high: wx.tempMax,
                              low: wx.tempMin,
                              chance: wx.wetChance,
                            })
                          : t('plan.weather.ribbonTypical', {
                              from: wx.years?.[0] ?? '',
                              to: wx.years?.[1] ?? '',
                              condition: t(WEATHER_KEY[wx.condition]),
                              high: wx.tempMax,
                              low: wx.tempMin,
                              chance: wx.wetChance,
                            })
                      }
                    >
                      <WeatherGlyph condition={wx.condition} label={t(WEATHER_KEY[wx.condition])} />
                      {wx.tempMax}°
                    </span>
                  )}
                </span>

                {feas && feas.feasibility !== 'ok' && (
                  <em className={`rb-flag ${feas.feasibility}`}>{t(FEASIBILITY_KEY[feas.feasibility])}</em>
                )}
              </span>
            </button>
          );
        })}
      </div>
    </section>
  );
}
