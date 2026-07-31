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
/**
 * Every path is drawn for 13px first and 24px second — the ribbon renders these
 * at 13 and the day canvas at 15, so a glyph that only resolves at 24 is a
 * glyph that fails everywhere it is actually used. That ruled out the bed
 * (four strokes that merge into a flag) and forced `visit` off the pitched-roof
 * outline it shared with `lodging`: two kinds cannot be the same silhouette.
 */
/** Exported because the map draws these too, and it draws them imperatively —
    MockMapRenderer builds DOM nodes, not JSX, so it takes the `d` and nothing
    else. One set of paths, two renderers. */
export const KIND_GLYPH_PATH: Record<StopKind, string> = {
  // Monument — plinth, three columns, pediment. Reads as "a thing you go and
  // look at" without belonging to any one country's architecture.
  visit: 'M2.5 21h19M5 21V10.5M12 21V10.5M19 21V10.5M3 10.5h18M12 3l-9 7.5M12 3l9 7.5',
  // Fork and knife.
  meal: 'M6.5 3v6a2.5 2.5 0 005 0V3M9 9.5V21M17.6 3c-1.7 1.4-2.4 3.3-2.4 5.4 0 1.7.9 2.8 2.4 3.1V21',
  // House with a door. The user's own suggestion, and the right one: a bed is
  // where you sleep, a house is where you stay.
  lodging: 'M2.8 11L12 3.6 21.2 11M5.2 9.9V20.5h13.6V9.9M9.9 20.5v-5.2h4.2v5.2',
  // Ticket — a booked, timed thing. One rounded outline and a perforation, no
  // side notches: the notches are what turned this into a grey smudge at 13px.
  activity:
    'M4 7.2h16a1.6 1.6 0 011.6 1.6v6.4a1.6 1.6 0 01-1.6 1.6H4a1.6 1.6 0 01-1.6-1.6V8.8A1.6 1.6 0 014 7.2zM14.6 8.4v1.5M14.6 11.3v1.5M14.6 14.2v1.5',
  // Tram front — body, window band, two wheels on the rail.
  transit:
    'M6.2 4.5h11.6v10.2a2.6 2.6 0 01-2.6 2.6H8.8a2.6 2.6 0 01-2.6-2.6zM6.2 10.2h11.6M9.4 20.6l-1.2-3.3M14.6 20.6l1.2-3.3M4.6 20.6h14.8',
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
      <path d={KIND_GLYPH_PATH[kind]} />
    </svg>
  );
}
