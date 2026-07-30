import { daySky } from '../lib/daylight';
import type { Day, PlanDetail, Stop } from '../api/types';

/**
 * The day's window painted as a sky: pale blue daylight, a golden-hour ramp
 * into dusk, indigo night, and a tick at the sunset moment. Computed locally
 * (no API). Hidden when the day has no located stops or no sunrise/sunset.
 *
 * The full-width day view paints this vertically *behind* the stops instead
 * (see DayCanvas), where it can put a stop below the sunset line rather than
 * merely next to it. This horizontal strip remains for the compact map panel,
 * which has no column to paint behind — both read the same `daySky`, so they
 * cannot disagree about when the sun goes down.
 */
export function DaylightStrip({ day, detail, stops }: { day: Day; detail: PlanDetail; stops: Stop[] }) {
  const sky = daySky(day, detail, stops);
  if (!sky) return null;
  const { rise, set, riseAt, setAt } = sky;

  return (
    <div
      className="daylight"
      title={`Daylight ${rise}–${set}, shown across the ${day.windowStart}–${day.windowEnd} window`}
    >
      <div className="track" style={{ background: `linear-gradient(90deg, ${sky.stops})` }}>
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
