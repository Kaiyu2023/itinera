import type { Leg, Place, PlaceKind, StopKind } from '../api/types';

/** Stop-kind hues, shared by timeline nodes, map pins, and panel cards. */
export const KIND_COLOR: Record<StopKind, string> = {
  visit: 'var(--color-kind-sight)',
  meal: 'var(--color-kind-food)',
  lodging: 'var(--color-kind-lodging)',
  activity: 'var(--color-kind-activity)',
  transit: 'var(--color-kind-transit)',
};

/** Built-in card labels; trips override per kind via Trip.stopKindLabels. */
export const KIND_LABEL: Record<StopKind, string> = {
  visit: 'visit',
  meal: 'meal',
  lodging: 'lodging',
  activity: 'activity',
  transit: 'transit',
};

export const MODE_ICON: Record<Leg['mode'], string> = { walk: '🚶', transit: '🚃', drive: '🚗', flight: '✈️' };

/** A place kind is a stop kind that hasn't been scheduled yet. One map, so a
    candidate and the stop it becomes wear the same glyph and the same label. */
export const PLACE_KIND_STOP_KIND: Record<PlaceKind, StopKind> = {
  sight: 'visit',
  food: 'meal',
  lodging: 'lodging',
  activity: 'activity',
  transport_hub: 'transit',
};

/** Place kinds map onto the same palette as the stop kinds they become. */
export const PLACE_KIND_COLOR: Record<PlaceKind, string> = {
  sight: KIND_COLOR.visit,
  food: KIND_COLOR.meal,
  lodging: KIND_COLOR.lodging,
  activity: KIND_COLOR.activity,
  transport_hub: KIND_COLOR.transit,
};

/** One-line leg chip for the compact map panel: first clause of the routing
    note, or the distance when the note is missing/unwieldy. */
export function shortLegLabel(leg: Leg): string {
  const head = `${MODE_ICON[leg.mode]} ${leg.durationMin} min`;
  if (leg.feasibilityNote) {
    let clause = leg.feasibilityNote.split(/[;—]/)[0].trim();
    if (clause.length > 42) clause = clause.split(/[.,]/)[0].trim();
    return `${head} — ${clause.replace(/\.$/, '')}`;
  }
  return `${head} · ${(leg.distanceM / 1000).toFixed(1)} km`;
}

/** Outbound "open in your maps app" link. The one provider-specific URL in the
    frontend, isolated here so a provider swap touches exactly one line. */
export function externalMapUrl(place: Place): string {
  return `https://www.google.com/maps/search/?api=1&query=${place.lat}%2C${place.lng}`;
}
