import { formatInTz, hhmmToMin, sunTimes } from '../lib/sun';
import type { Day, PlanDetail, Stop } from '../api/types';

/** Sky colors for the daylight strip — dawn/dusk glow between day and night. */
const SKY = { night: '#353c66', plum: '#8a6488', glow: '#e8935a', gold: '#f4cf7a', day: '#dce8f2' };

/**
 * The day's window painted as a sky: pale blue daylight, a golden-hour ramp
 * into dusk, indigo night, and a tick at the sunset moment. Computed locally
 * (no API). Hidden when the day has no located stops or no sunrise/sunset.
 */
export function DaylightStrip({ day, detail, stops }: { day: Day; detail: PlanDetail; stops: Stop[] }) {
  const anchor = stops.map((s) => detail.places.find((p) => p.id === s.placeId)).find(Boolean);
  if (!anchor) return null;
  const sun = sunTimes(day.date, anchor.lat, anchor.lng);
  if (!sun) return null;

  const rise = formatInTz(sun.sunrise, day.tz);
  const set = formatInTz(sun.sunset, day.tz);
  const windowStart = hhmmToMin(day.windowStart);
  const windowEnd = hhmmToMin(day.windowEnd);
  const span = windowEnd - windowStart;
  // Unclamped window fractions — negative/over-100 means outside the window.
  const riseAt = ((hhmmToMin(rise) - windowStart) / span) * 100;
  const setAt = ((hhmmToMin(set) - windowStart) / span) * 100;

  const c = (n: number) => Math.max(0, Math.min(100, n));
  const g: string[] = [];
  if (riseAt > -6) {
    g.push(
      `${SKY.night} ${c(riseAt - 8)}%`,
      `${SKY.plum} ${c(riseAt - 3)}%`,
      `${SKY.glow} ${c(riseAt)}%`,
      `${SKY.gold} ${c(riseAt + 3)}%`,
      `${SKY.day} ${c(riseAt + 12)}%`,
    );
  } else {
    g.push(`${SKY.day} 0%`);
  }
  if (setAt < 106) {
    g.push(
      `${SKY.day} ${c(setAt - 12)}%`,
      `${SKY.gold} ${c(setAt - 3)}%`,
      `${SKY.glow} ${c(setAt + 1)}%`,
      `${SKY.plum} ${c(setAt + 4)}%`,
      `${SKY.night} ${c(setAt + 9)}%`,
    );
  } else {
    g.push(`${SKY.day} 100%`);
  }

  return (
    <div
      className="daylight"
      title={`Daylight ${rise}–${set}, shown across the ${day.windowStart}–${day.windowEnd} window`}
    >
      <div className="track" style={{ background: `linear-gradient(90deg, ${g.join(', ')})` }}>
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
