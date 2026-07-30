import type { StopKind } from '../api/types';

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
const PATHS: Record<StopKind, string> = {
  // temple gate / monument
  visit: 'M4 21h16M6 21V10l6-4 6 4v11M10 21v-6h4v6',
  // fork and knife
  meal: 'M7 3v7a2 2 0 004 0V3M9 10v11M17 3c-1.4 1.2-2 3-2 5s.6 2.8 2 3v10',
  // bed
  lodging: 'M3 20V8M3 13h13a4 4 0 014 4v3M3 20h18M7 10.5h2.5',
  // ticket
  activity:
    'M4 8a2 2 0 012-2h12a2 2 0 012 2 2 2 0 000 4 2 2 0 000 4 2 2 0 01-2 2H6a2 2 0 01-2-2 2 2 0 000-4 2 2 0 000-4M13 7v10',
  // train
  transit:
    'M7 4h10a2 2 0 012 2v9a3 3 0 01-3 3H8a3 3 0 01-3-3V6a2 2 0 012-2M5 10h14M9 18l-2 3M15 18l2 3M9 14h.01M15 14h.01',
};

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
      <path d={PATHS[kind]} />
    </svg>
  );
}
