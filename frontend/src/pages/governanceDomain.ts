import type { Day, Feasibility, PlaceKind, Stop } from '../api/types';
import type { planEnglish } from '../i18n/messages.plan';

export type PlanTranslate = (key: keyof typeof planEnglish, values?: Record<string, string | number>) => string;

/** Stable option order for the localized place-kind picker. */
export const PLACE_KINDS: readonly PlaceKind[] = ['sight', 'food', 'lodging', 'activity', 'transport_hub'];

const PLACE_KIND_KEY = {
  sight: 'plan.gov.placeKind.sight',
  food: 'plan.gov.placeKind.food',
  lodging: 'plan.gov.placeKind.lodging',
  activity: 'plan.gov.placeKind.activity',
  transport_hub: 'plan.gov.placeKind.transport',
} as const;

export function localizedPlaceKind(kind: PlaceKind, translate: PlanTranslate): string {
  return translate(PLACE_KIND_KEY[kind]);
}

/** Options for inserting a stop first or after one of the day's existing stops. */
export function slotOptions(
  stops: Stop[],
  placeName: (placeId: string) => string,
  translate: PlanTranslate,
): { value: string; label: string }[] {
  return [
    { value: 'first', label: translate('plan.gov.slotFirst') },
    ...stops.map((stop) => ({
      value: stop.id,
      label: translate('plan.gov.slotAfter', { place: placeName(stop.placeId) }),
    })),
  ];
}

/** Fractional sequence used until the API normalizes stop ordering. */
export function seqForSlot(value: string, stops: Stop[]): number {
  if (value === 'first') return 0.5;
  const precedingStop = stops.find((stop) => stop.id === value);
  return precedingStop ? precedingStop.seq + 0.5 : stops.length + 1;
}

export const NEW_STOP_VISIT_MIN = 60;

export interface ProjectedFeasibility {
  feasibility: Feasibility;
  pct: number;
}

/** Project a day's feasibility after adding the mock API's default one-hour stop. */
export function projectFeasibilityAfterAdd(usedMin: number, windowMin: number): ProjectedFeasibility {
  const pct = (usedMin + NEW_STOP_VISIT_MIN) / windowMin;
  const feasibility: Feasibility = pct > 1 ? 'unreasonable' : pct >= 0.85 ? 'tight' : 'ok';
  return { feasibility, pct };
}

/** Day dropdown label used by proposal composers. */
export function dayOptionLabel(
  day: Day,
  index: number,
  formatDate: (iso: string, options?: Intl.DateTimeFormatOptions) => string,
  translate: PlanTranslate,
): string {
  const date = formatDate(day.date, { weekday: 'short', day: 'numeric' });
  return translate('plan.gov.dayOption', { day: index + 1, date, city: day.cityHint });
}
