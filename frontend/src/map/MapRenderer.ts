/**
 * MapRenderer — the port between trip UIs and any map provider (DESIGN.md §4.1).
 *
 * The UI never talks to a map SDK directly; it hands this interface plain
 * geographic data (markers, routes, bounds) and lets the renderer draw it.
 * Phase A ships MockMapRenderer (keyless, offline, stylised placeholder
 * tiles); Phase B adds GoogleMapRenderer behind the same interface without
 * touching a single caller.
 *
 * Provider-native chrome (zoom control, attribution, panning) is each
 * renderer's own responsibility — Google Maps brings its own, the mock draws
 * its own. App-level overlays (candidate toggles, stop popovers) are built by
 * the UI on top, positioned via `project()`.
 *
 * An app overlay that permanently covers part of the frame should carry
 * `data-map-chrome`: the renderer decides in screen space where marker name
 * tags can go, and a tag placed under a floating toggle is a tag nobody reads.
 */

export interface LngLat {
  lng: number;
  lat: number;
}

export interface LngLatBounds {
  west: number;
  south: number;
  east: number;
  north: number;
}

export type MarkerVariant =
  | 'stop' // pin: feasibility fill, kind glyph, sequence badge
  | 'candidate' // hollow ring — shortlisted, not in the plan
  | 'home' // rounded square — the night's lodging, carried in
  | 'city' // small dot with a display-face name, trip overview
  | 'transport' // small glyph pin (✈ …)
  | 'search-result' // a place-search hit — a dot with a soft halo
  | 'bead' // small numbered dot — "day 3 happens here", trip overview
  | 'chip'; // floating label only, no pin (e.g. "Days 1–3 · Tokyo")

export interface MapMarker {
  id: string;
  position: LngLat;
  variant: MarkerVariant;
  /** Pin colour (any CSS colour). Unused by `chip`.
      In the plan this is the ALARM channel and nothing else: a stop pin is
      neutral ink unless one of its legs is tight/unreasonable/impossible. */
  color?: string;
  /** Short text glyph inside the pin — ✈, a day number. */
  label?: string;
  /** SVG path `d` drawn inside the pin — how *kind* is carried now that it
      has given up colour (see components/KindGlyph.tsx for the paths). */
  glyphPath?: string;
  /** Sequence badge riding the pin's shoulder ("3" = third stop of the day). */
  seq?: number;
  /** Floating name tag beside the pin. */
  tag?: string;
  /** Preferred side. The renderer may flip it to keep the tag on screen and
      off its neighbours — screen-space collisions are only knowable there. */
  tagPlacement?: 'below' | 'above' | 'left' | 'right';
  selected?: boolean;
  /** Default true; false renders without click affordance or events. */
  interactive?: boolean;
  /** Screen-reader name. Falls back to tag/label — set it when neither says
      enough on its own ("3, Omoide Yokocho, meal, leg is tight"). */
  ariaLabel?: string;
}

export interface MapRoute {
  id: string;
  points: LngLat[];
  color: string;
  dashed?: boolean;
  /** Stroke width in px (default 4). Severity is carried in weight as well as
      in hue, so an unreasonable leg is drawn thicker than a fine one. */
  width?: number;
}

/** Per-edge pixel padding for `fitBounds`.
 *
 * A single scalar cannot describe a map that is 46% covered by a bottom sheet
 * and 96px covered by floating chrome at the top: it either wastes the frame
 * or hides the geometry under the furniture. Every caller that has chrome over
 * its map passes the four edges separately. */
export interface EdgePadPx {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/** Localized provider chrome supplied by the React UI layer. */
export interface MapUiLabels {
  zoomIn: string;
  zoomOut: string;
  attribution: string;
}

export interface MapRenderer {
  mount(container: HTMLElement): void;
  destroy(): void;
  setMarkers(markers: MapMarker[]): void;
  setRoutes(routes: MapRoute[]): void;
  /** Update provider-owned labels without rebuilding or resetting the map. */
  setUiLabels(labels: MapUiLabels): void;
  /** Fit the view to bounds inside a pixel padding (default 24), which may be
      one scalar or four edges. Resets zoom. */
  fitBounds(bounds: LngLatBounds, padding?: number | EdgePadPx): void;
  /** Geographic → container-pixel. Null before mount/fitBounds. */
  project(position: LngLat): { x: number; y: number } | null;
  onMarkerClick(handler: ((markerId: string) => void) | null): void;
  onMapClick(handler: (() => void) | null): void;
  /** Fired after zoom/pan/resize/fit — overlays re-run `project()`. */
  onViewChange(handler: (() => void) | null): void;
}
