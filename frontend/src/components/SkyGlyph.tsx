/**
 * The sky's own iconography: the two horizons, and the weather.
 *
 * Drawn rather than typed. The daylight strip used to say `sunset 16:35 ☀ ↓`,
 * which is three symbols doing one job and none of them meaning "night" — the
 * sun glyph was still the sun at the moment the sun goes away. A sun for the
 * light half and a crescent-with-a-star for the dark half is the oldest
 * shorthand there is, and unlike a hue it survives a dichromat, a greyscale
 * print and a 13px render.
 *
 * All paths are stroke-only on a 24-box so they inherit `currentColor` and
 * `stroke-width` from whatever they land in.
 */

function Svg({ children, label, cls }: { children: React.ReactNode; label?: string; cls: string }) {
  return (
    <svg
      className={cls}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {children}
    </svg>
  );
}

/** Sunrise. A disc with eight rays — deliberately the busiest glyph here, so
    the moment the day opens is the loudest mark on the rail. */
export function SunGlyph({ label }: { label?: string }) {
  return (
    <Svg cls="sky-glyph sun" label={label}>
      <circle cx="12" cy="12" r="4.2" />
      <path d="M12 2.5v2.4M12 19.1v2.4M2.5 12h2.4M19.1 12h2.4M5.2 5.2l1.7 1.7M17.1 17.1l1.7 1.7M18.8 5.2l-1.7 1.7M6.9 17.1l-1.7 1.7" />
    </Svg>
  );
}

/** Sunset. Crescent plus a star — the user's own shorthand for "this is the
    part of the day you need a jacket for". */
export function MoonGlyph({ label }: { label?: string }) {
  return (
    <Svg cls="sky-glyph moon" label={label}>
      <path d="M20 14.4A8.2 8.2 0 019.6 4a8.6 8.6 0 1010.4 10.4z" />
      <path d="M18.4 3.2l.75 1.85 1.85.75-1.85.75-.75 1.85-.75-1.85L15.8 5.8l1.85-.75z" />
    </Svg>
  );
}

/* ---- Weather ------------------------------------------------------------ */

const CLOUD = 'M7.2 18.5h9.4a3.6 3.6 0 00.5-7.16 5.2 5.2 0 00-9.86-1.5A3.83 3.83 0 007.2 18.5z';
/** The cloud lifted clear of the bottom third, where precipitation goes. */
const CLOUD_HIGH = 'M7.2 14.5h9.4a3.6 3.6 0 00.5-7.16 5.2 5.2 0 00-9.86-1.5A3.83 3.83 0 007.2 14.5z';

/** A small sun peeking out behind the cloud, for `partly`. */
const SUN_BEHIND = 'M14.4 5.9a3.2 3.2 0 013.9 3.9M15.6 2.6v1.6M20.4 4.1l-1.1 1.1M22 8.9h-1.6';

const CONDITION_PATHS: Record<SkyCondition, React.ReactNode> = {
  clear: (
    <>
      <circle cx="12" cy="12" r="4.2" />
      <path d="M12 3.4v1.6M12 19v1.6M3.4 12H5M19 12h1.6M6 6l1.2 1.2M16.8 16.8L18 18M18 6l-1.2 1.2M7.2 16.8L6 18" />
    </>
  ),
  partly: (
    <>
      <path d={SUN_BEHIND} />
      <path d={CLOUD} />
    </>
  ),
  cloud: (
    <>
      <path d={CLOUD} />
      <path d="M4.6 8.4a3.6 3.6 0 013.2-3" opacity="0.55" />
    </>
  ),
  fog: (
    <>
      <path d={CLOUD_HIGH} />
      <path d="M4.5 18h11M7 21h11" />
    </>
  ),
  drizzle: (
    <>
      <path d={CLOUD_HIGH} />
      <path d="M9 17.5v1.6M13 17.5v1.6M17 17.5v1.6" />
    </>
  ),
  rain: (
    <>
      <path d={CLOUD_HIGH} />
      <path d="M8.5 17l-1.2 4M12.6 17l-1.2 4M16.7 17l-1.2 4" />
    </>
  ),
  snow: (
    <>
      <path d={CLOUD_HIGH} />
      <path d="M9 18.4v3M7.7 19.1l2.6 1.5M10.3 19.1l-2.6 1.5M16 18.4v3M14.7 19.1l2.6 1.5M17.3 19.1l-2.6 1.5" />
    </>
  ),
  storm: (
    <>
      <path d={CLOUD_HIGH} />
      <path d="M13.4 16.4l-3.6 3.4h3.2L11.6 23" />
    </>
  ),
};

export function WeatherGlyph({ condition, label }: { condition: SkyCondition; label?: string }) {
  return (
    <Svg cls={`sky-glyph wx wx-${condition}`} label={label}>
      {CONDITION_PATHS[condition]}
    </Svg>
  );
}
import type { SkyCondition } from './skyConditions';
