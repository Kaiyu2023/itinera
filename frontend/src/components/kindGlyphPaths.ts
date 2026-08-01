import type { StopKind } from '../api/types';

/** Shared SVG paths used by both React and the imperative map renderer. */
export const KIND_GLYPH_PATH: Record<StopKind, string> = {
  visit: 'M2.5 21h19M5 21V10.5M12 21V10.5M19 21V10.5M3 10.5h18M12 3l-9 7.5M12 3l9 7.5',
  meal: 'M6.5 3v6a2.5 2.5 0 005 0V3M9 9.5V21M17.6 3c-1.7 1.4-2.4 3.3-2.4 5.4 0 1.7.9 2.8 2.4 3.1V21',
  lodging: 'M2.8 11L12 3.6 21.2 11M5.2 9.9V20.5h13.6V9.9M9.9 20.5v-5.2h4.2v5.2',
  activity:
    'M4 7.2h16a1.6 1.6 0 011.6 1.6v6.4a1.6 1.6 0 01-1.6 1.6H4a1.6 1.6 0 01-1.6-1.6V8.8A1.6 1.6 0 014 7.2zM14.6 8.4v1.5M14.6 11.3v1.5M14.6 14.2v1.5',
  transit:
    'M6.2 4.5h11.6v10.2a2.6 2.6 0 01-2.6 2.6H8.8a2.6 2.6 0 01-2.6-2.6zM6.2 10.2h11.6M9.4 20.6l-1.2-3.3M14.6 20.6l1.2-3.3M4.6 20.6h14.8',
};
