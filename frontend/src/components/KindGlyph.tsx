import type { StopKind } from '../api/types';
import { KIND_GLYPH_PATH } from './kindGlyphPaths';

/**
 * Stop kind as a shape rather than a colour.
 *
 * Colour's one job in the plan is the alarm channel (feasibility). Kind used to
 * compete for it, which is how an orange "MEAL" chip ended up sitting directly
 * above an orange "optimistic" leg warning — a category and an alarm rendered
 * in the same ink. Kind gives colour up and takes a glyph instead.
 *
 * This is strictly more legible, not a consolation prize: at five categories
 * the old hues were not separable by a dichromat anyway (activity vs transit
 * measured 1.03:1 under simulated deuteranopia, i.e. identical), whereas a
 * shape works for everyone.
 */
/**
 * Every path is drawn for 13px first and 24px second — the ribbon renders these
 * at 13 and the day canvas at 15, so a glyph that only resolves at 24 is a
 * glyph that fails everywhere it is actually used. That ruled out the bed
 * (four strokes that merge into a flag) and forced `visit` off the pitched-roof
 * outline it shared with `lodging`: two kinds cannot be the same silhouette.
 */
export function KindGlyph({ kind, label }: { kind: StopKind; label?: string }) {
  return (
    <svg
      className="kind-glyph"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <path d={KIND_GLYPH_PATH[kind]} />
    </svg>
  );
}
