import type { Leg, Place, PlaceKind, StopKind } from '../api/types';
import type { planEnglish } from '../i18n/messages.plan';
import { formatPlanDistance, formatPlanDuration } from '../i18n/messages.plan';

/** Stop-kind hues, shared by timeline nodes, map pins, and panel cards. */
export const KIND_COLOR: Record<StopKind, string> = {
  visit: 'var(--color-kind-sight)',
  meal: 'var(--color-kind-food)',
  lodging: 'var(--color-kind-lodging)',
  activity: 'var(--color-kind-activity)',
  transit: 'var(--color-kind-transit)',
};

export const MODE_ICON: Record<Leg['mode'], string> = { walk: '🚶', transit: '🚃', drive: '🚗', flight: '✈️' };

const MODE_KEY = {
  walk: 'plan.mode.walk',
  transit: 'plan.mode.transit',
  drive: 'plan.mode.drive',
  flight: 'plan.mode.flight',
} as const;

type PlanTranslate = (key: keyof typeof planEnglish, values?: Record<string, string | number>) => string;

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
export function shortLegLabel(leg: Leg, locale: 'en' | 'zh-CN', t: PlanTranslate): string {
  const head = `${MODE_ICON[leg.mode]} ${t(MODE_KEY[leg.mode])} · ${formatPlanDuration(leg.durationMin, t)}`;
  if (leg.feasibilityNote) {
    let clause = leg.feasibilityNote.split(/[;—]/)[0].trim();
    if (clause.length > 42) clause = clause.split(/[.,]/)[0].trim();
    return `${head} — ${clause.replace(/\.$/, '')}`;
  }
  return `${head} · ${formatPlanDistance(leg.distanceM, locale, t)}`;
}

/** Outbound "open in your maps app" link. The one provider-specific URL in the
    frontend, isolated here so a provider swap touches exactly one line. */
export function externalMapUrl(place: Place): string {
  return `https://www.google.com/maps/search/?api=1&query=${place.lat}%2C${place.lng}`;
}
