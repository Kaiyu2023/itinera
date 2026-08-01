import { useCallback, useSyncExternalStore } from 'react';
import { coreEnglish, coreSimplifiedChinese } from './catalogs';
import { tripListCatalog } from './catalogs/tripList';
import { moneyPrepEnglish, moneyPrepSimplifiedChinese } from './messages.moneyPrep';
import { planEnglish, planSimplifiedChinese } from './messages.plan';
import { socialEnglish, socialSimplifiedChinese } from './messages.social';

/**
 * UI language only.
 *
 * Messages describe product chrome, labels and generated helper text. Trip
 * names, place names, notes, discussions and every other user-authored or
 * API-provided string must be rendered as-is rather than passed through `t`.
 */

export type UiLocale = 'en' | 'zh-CN';
export const UI_LOCALES: readonly UiLocale[] = ['en', 'zh-CN'];

// Feature catalogs can be spread into these two assemblies independently. The
// mapped Chinese type makes a missing translation a compile-time error.
const englishMessages = {
  ...coreEnglish,
  ...tripListCatalog.en,
  ...moneyPrepEnglish,
  ...planEnglish,
  ...socialEnglish,
} as const;
export type MessageKey = keyof typeof englishMessages;
const simplifiedChineseMessages: Record<MessageKey, string> = {
  ...coreSimplifiedChinese,
  ...tripListCatalog['zh-CN'],
  ...moneyPrepSimplifiedChinese,
  ...planSimplifiedChinese,
  ...socialSimplifiedChinese,
};

function placeholders(template: string): string[] {
  return [...template.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
}

// Key parity is checked by TypeScript; interpolation parity needs this small
// runtime guard so a translated template cannot silently lose or rename a
// value. It runs once when the catalog module loads.
for (const key of Object.keys(englishMessages) as MessageKey[]) {
  const source = placeholders(englishMessages[key]);
  const translated = placeholders(simplifiedChineseMessages[key]);
  if (source.join('\0') !== translated.join('\0')) {
    throw new Error(`Translation placeholders do not match for "${key}".`);
  }
}

const messages: Record<UiLocale, Record<MessageKey, string>> = {
  en: englishMessages,
  'zh-CN': simplifiedChineseMessages,
};

export type TranslationValues = Record<string, string | number>;

const STORAGE_KEY = 'itinera.locale';

function isUiLocale(value: string | null): value is UiLocale {
  return value === 'en' || value === 'zh-CN';
}

function localeFromLanguage(language: string): UiLocale | null {
  const normalized = language.toLowerCase().replaceAll('_', '-');
  if (normalized === 'en' || normalized.startsWith('en-')) return 'en';
  if (
    normalized === 'zh' ||
    normalized === 'zh-cn' ||
    normalized.startsWith('zh-cn-') ||
    normalized === 'zh-sg' ||
    normalized.startsWith('zh-sg-') ||
    normalized === 'zh-hans' ||
    normalized.startsWith('zh-hans-')
  )
    return 'zh-CN';
  return null;
}

function negotiateBrowserLocale(): UiLocale {
  if (typeof navigator === 'undefined') return 'en';
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
  // Respect preference order. `some(zh)` incorrectly chose Chinese for an
  // English-first browser that merely listed Chinese as a secondary language.
  for (const language of languages) {
    const supported = localeFromLanguage(language);
    if (supported) return supported;
  }
  return 'en';
}

function readLocale(): UiLocale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (isUiLocale(saved)) return saved;
  } catch {
    /* A blocked storage API should not prevent language negotiation. */
  }
  return negotiateBrowserLocale();
}

let currentLocale: UiLocale = typeof localStorage === 'undefined' ? 'en' : readLocale();
const listeners = new Set<() => void>();

function applyLocale(locale: UiLocale) {
  if (typeof document === 'undefined') return;
  document.documentElement.lang = locale;
  document.documentElement.dataset.locale = locale;
}

applyLocale(currentLocale);

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function setUiLocale(next: UiLocale) {
  if (next === currentLocale) {
    applyLocale(next);
    return;
  }
  currentLocale = next;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* Private browsing may block storage; the in-memory choice still works. */
  }
  applyLocale(next);
  for (const listener of listeners) listener();
}

if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== STORAGE_KEY || !isUiLocale(event.newValue) || event.newValue === currentLocale) return;
    currentLocale = event.newValue;
    applyLocale(currentLocale);
    for (const listener of listeners) listener();
  });
}

export function getUiLocale(): UiLocale {
  return currentLocale;
}

export function useUiLocale(): UiLocale {
  return useSyncExternalStore(subscribe, getUiLocale, () => 'en');
}

export function translate(locale: UiLocale, key: MessageKey, values?: TranslationValues): string {
  const template = messages[locale][key];
  if (!values) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = values[name];
    return value === undefined ? match : String(value);
  });
}

/** Stable, typed translation and date helpers for any UI component. */
export function useI18n() {
  const locale = useUiLocale();
  const t = useCallback((key: MessageKey, values?: TranslationValues) => translate(locale, key, values), [locale]);
  const formatDate = useCallback(
    (iso: string, options?: Intl.DateTimeFormatOptions) => formatUiDate(iso, locale, options),
    [locale],
  );
  const formatNumber = useCallback(
    (value: number, options?: Intl.NumberFormatOptions) => formatUiNumber(value, locale, options),
    [locale],
  );
  return { locale, setLocale: setUiLocale, t, formatDate, formatNumber };
}

export function formatUiDate(
  iso: string,
  locale: UiLocale,
  options: Intl.DateTimeFormatOptions = { weekday: 'short', month: 'short', day: 'numeric' },
): string {
  const value = new Date(iso + (iso.length === 10 ? 'T00:00:00' : ''));
  return new Intl.DateTimeFormat(locale, options).format(value);
}

export function formatUiNumber(value: number, locale: UiLocale, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(locale, options).format(value);
}

export interface LocalizedTripPhase {
  phase: 'before' | 'during' | 'after';
  label: string;
  short: string;
}

/** Generated trip-relative labels are UI chrome, so they follow the UI locale. */
export function getLocalizedTripPhase(startDate: string, endDate: string, locale: UiLocale): LocalizedTripPhase {
  const dayMs = 86_400_000;
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const start = new Date(`${startDate}T00:00:00`).getTime();
  const end = new Date(`${endDate}T00:00:00`).getTime();
  if (today < start) {
    const days = Math.round((start - today) / dayMs);
    const count = formatUiNumber(days, locale);
    return {
      phase: 'before',
      label:
        days === 1 ? translate(locale, 'trip.phase.tomorrow') : translate(locale, 'trip.phase.daysToGo', { count }),
      short: translate(locale, 'trip.phase.shortDays', { count }),
    };
  }
  if (today > end) return { phase: 'after', label: translate(locale, 'trip.phase.complete'), short: '✓' };
  const day = Math.round((today - start) / dayMs) + 1;
  const total = Math.round((end - start) / dayMs) + 1;
  return {
    phase: 'during',
    label: translate(locale, 'trip.phase.dayOf', {
      day: formatUiNumber(day, locale),
      total: formatUiNumber(total, locale),
    }),
    short: translate(locale, 'trip.phase.shortDay', { day: formatUiNumber(day, locale) }),
  };
}
