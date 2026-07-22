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
  | 'stop' // numbered, kind-coloured pin
  | 'candidate' // hollow ring — shortlisted, not in the plan
  | 'home' // rounded-square ⌂ — the night's lodging
  | 'city' // small dot with a display-face name, trip overview
  | 'transport' // small glyph pin (✈ …)
  | 'search-result' // a place-search hit — kind-coloured dot with a soft halo
  | 'chip'; // floating label only, no pin (e.g. "Days 1–3 · Tokyo")

export interface MapMarker {
  id: string;
  position: LngLat;
  variant: MarkerVariant;
  /** Pin colour (any CSS colour). Unused by `chip`. */
  color?: string;
  /** Short glyph inside the pin — a stop number, ⌂, ✈. */
  label?: string;
  /** Floating name tag beside the pin. */
  tag?: string;
  tagPlacement?: 'below' | 'above' | 'left';
  selected?: boolean;
  /** Default true; false renders without click affordance or events. */
  interactive?: boolean;
}

export interface MapRoute {
  id: string;
  points: LngLat[];
  color: string;
  dashed?: boolean;
}

export interface MapRenderer {
  mount(container: HTMLElement): void;
  destroy(): void;
  setMarkers(markers: MapMarker[]): void;
  setRoutes(routes: MapRoute[]): void;
  /** Fit the view to bounds with a pixel padding (default 24). Resets zoom. */
  fitBounds(bounds: LngLatBounds, padding?: number): void;
  /** Geographic → container-pixel. Null before mount/fitBounds. */
  project(position: LngLat): { x: number; y: number } | null;
  onMarkerClick(handler: ((markerId: string) => void) | null): void;
  onMapClick(handler: (() => void) | null): void;
  /** Fired after zoom/pan/resize/fit — overlays re-run `project()`. */
  onViewChange(handler: (() => void) | null): void;
}
