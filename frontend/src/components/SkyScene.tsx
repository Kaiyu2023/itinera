import type { CSSProperties } from 'react';

/**
 * The sky as a picture rather than a gradient with an icon parked on it.
 *
 * The band behind a day was doing the work with colour alone: a ramp from pale
 * blue to indigo, plus a 14px sun and a 14px crescent dropped on top like map
 * pins. That reads as a chart with decoration, and it makes the two halves of a
 * day distinguishable only by how dark they are — which is precisely the
 * comparison a small band is worst at. A sun with rays and drifting clouds, and
 * then stars and a moon, is a distinction you can make at a glance and across
 * the room, and it survives greyscale.
 *
 * Everything is placed against the same time axis the stops are on, so this is
 * not wallpaper: the sun sits at *solar noon*, the clouds occupy the daylight
 * span, the stars stop at the horizons. The scene is a second, redundant
 * encoding of exactly the data the wash already carries.
 *
 * Works on both axes. The ribbon runs left→right, the day canvas top→bottom.
 */

/**
 * Deterministic from the seed, so a day's stars stay where they were between
 * renders. `Math.random` would reshuffle the field on every keystroke elsewhere
 * in the app, which is both distracting and impossible to screenshot twice.
 * (Mulberry32 over an FNV-1a hash of the seed.)
 */
function rng(seed: string): () => number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return () => {
    h = (h + 0x6d2b79f5) | 0;
    let t = h;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface SkySceneProps {
  /** Which way time runs. */
  axis: 'x' | 'y';
  /** Sunrise, sunset and solar noon as fractions of the band. May fall outside
      0–1: a window that opens after sunrise has `rise < 0`, and the scene has
      to handle that rather than clamp it, or the stars creep into the morning. */
  rise: number;
  set: number;
  noon: number;
  /** Stable identity for the field — the day's id will do. */
  seed: string;
  /** How many candidate marks to scatter before filtering by daylight. Scale it
      with the band's length in pixels, not with anything semantic. */
  density?: number;
  /** How far across the *other* axis marks may sit, as fractions. The ribbon
      keeps them clear of the stop line; the canvas uses the full width. */
  cross?: [number, number];
  /** Diameter of the sun and the moon, in px. */
  bodySize?: number;
  /** Cloud width range in px. A 40px cloud is weather in a 58px ribbon band and
      a speck on a 1200px column, so the caller sets it. */
  cloudSize?: [number, number];
  /** Star width range in px. These are drawn shapes, not dots, so they need
      real size to read as stars — under about 4px a four-point star is a
      smudge, which is the whole reason the first version used circles. */
  starSize?: [number, number];
}

/** Twilight is where the stars fade in, not a hard edge. */
const FADE = 0.055;

/**
 * The four-point star, with concave sides so the points read as points at
 * 6px. Round dots were technically more like real stars and looked like dust;
 * this is the shape everybody actually draws when they draw a night sky.
 */
const STAR = 'M12 1.6l2.35 7.4 7.4 2.35-7.4 2.35-2.35 7.4-2.35-7.4-7.4-2.35 7.4-2.35z';

export function SkyScene({
  axis,
  rise,
  set,
  noon,
  seed,
  density = 26,
  cross = [0.08, 0.92],
  bodySize = 20,
  cloudSize = [18, 34],
  starSize = [5, 11],
}: SkySceneProps) {
  const rand = rng(seed);
  const at = (p: number, c: number): CSSProperties =>
    axis === 'x' ? { left: `${p * 100}%`, top: `${c * 100}%` } : { top: `${p * 100}%`, left: `${c * 100}%` };
  const crossAt = () => cross[0] + rand() * (cross[1] - cross[0]);

  /** 0 in full daylight, 1 in full night, ramped across twilight. */
  const darkness = (p: number) => {
    if (p >= rise + FADE && p <= set - FADE) return 0;
    if (p <= rise - FADE || p >= set + FADE) return 1;
    const edge = p < (rise + set) / 2 ? rise : set;
    return Math.min(1, Math.abs(p - edge) / (2 * FADE));
  };

  // The sun goes at solar noon when the band contains it, and otherwise at the
  // middle of whatever daylight is visible — a window that opens at 14:00 still
  // has a sun in it, it is just not overhead.
  const litFrom = Math.max(0, rise);
  const litTo = Math.min(1, set);
  const sunAt = noon > 0.04 && noon < 0.96 ? noon : (litFrom + litTo) / 2;
  const showSun = litTo - litFrom > 0.12;

  const stars: Array<{ p: number; c: number; w: number; a: number; rot: number }> = [];
  const clouds: Array<{ p: number; c: number; w: number; a: number }> = [];
  for (let i = 0; i < density; i++) {
    const p = rand();
    const c = crossAt();
    const dark = darkness(p);
    if (dark > 0.12) {
      // Size is skewed small — `rand()` squared — so a field is mostly modest
      // stars with the occasional big one, rather than a uniform sprinkle of
      // identical shapes, which reads as a texture rather than a sky.
      const t = rand() ** 2;
      stars.push({
        p,
        c,
        w: starSize[0] + t * (starSize[1] - starSize[0]),
        a: (0.45 + rand() * 0.55) * dark,
        // A four-point star is symmetric every 90°, so this is the full range
        // of distinct orientations.
        rot: rand() * 90,
      });
      continue;
    }
    // Fewer clouds than stars, and none parked on the sun. An overcast band
    // buries the stop line, and a cloud sitting on the disc costs the day half
    // the one mark that makes it legible at a glance.
    if (rand() > 0.44) continue;
    if (showSun && Math.abs(p - sunAt) < 0.09) continue;
    clouds.push({ p, c, w: cloudSize[0] + rand() * (cloudSize[1] - cloudSize[0]), a: 0.5 + rand() * 0.5 });
  }

  // The moon takes the longer of the two night stretches.
  const preDawn = Math.max(0, Math.min(1, rise));
  const postDusk = 1 - Math.max(0, Math.min(1, set));
  const moonAt = preDawn > postDusk ? preDawn / 2 : 1 - postDusk / 2;
  const showMoon = Math.max(preDawn, postDusk) > 0.14;

  return (
    <span className={`sky-scene ax-${axis}`} aria-hidden>
      {clouds.map((cl, i) => (
        <span key={`c${i}`} className="sc-cloud" style={{ ...at(cl.p, cl.c), width: `${cl.w}px`, opacity: cl.a }}>
          <svg viewBox="0 0 24 16" fill="currentColor">
            <path d="M18.4 15H7.1a5 5 0 0 1-.6-9.96A6.6 6.6 0 0 1 18.1 4.3a5.4 5.4 0 0 1 .3 10.7z" />
          </svg>
        </span>
      ))}

      {/* Mid-band on both axes. On the ribbon the disc used to sit at 0.26,
          which put it directly behind the label plate — the one mark that makes
          a day legible at a glance, hidden behind its own caption. */}
      {showSun && (
        <span className="sc-sun" style={{ ...at(sunAt, axis === 'x' ? 0.47 : 0.5), width: bodySize, height: bodySize }}>
          <span className="sc-disc" />
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
            <path d="M12 1.2v2.6M12 20.2v2.6M1.2 12h2.6M20.2 12h2.6M4.4 4.4l1.9 1.9M17.7 17.7l1.9 1.9M19.6 4.4l-1.9 1.9M6.3 17.7l-1.9 1.9" />
          </svg>
        </span>
      )}

      {stars.map((s, i) => (
        <span
          key={`s${i}`}
          className="sc-star"
          style={{
            ...at(s.p, s.c),
            width: s.w,
            height: s.w,
            opacity: s.a,
            transform: `translate(-50%, -50%) rotate(${s.rot.toFixed(1)}deg)`,
          }}
        >
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d={STAR} />
          </svg>
        </span>
      ))}

      {showMoon && (
        <span
          className="sc-moon"
          style={{ ...at(moonAt, axis === 'x' ? 0.49 : 0.52), width: bodySize, height: bodySize }}
        >
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
          </svg>
          {/* The star beside the crescent. Not astronomy — it is the shorthand
              everyone already reads as "night", which is the whole job. */}
          <svg className="sc-spark" viewBox="0 0 24 24" fill="currentColor">
            <path d={STAR} />
          </svg>
        </span>
      )}
    </span>
  );
}
