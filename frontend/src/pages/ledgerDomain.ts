import type { ExpenseCategory, ExpenseSplit, StopKind, User } from '../api/types';
import type { UiLocale } from '../i18n';
import { formatUiNumber } from '../i18n';
import type { MoneyPrepMessageKey } from '../i18n/messages.moneyPrep';

type LedgerTranslate = (key: MoneyPrepMessageKey, values?: Record<string, string | number>) => string;

export interface StopOption {
  id: string;
  label: string;
  stopKind: StopKind;
  note: string;
}

export interface Transfer {
  fromUser: string;
  toUser: string;
  amount: number;
}

export const CATEGORY_META: Record<ExpenseCategory, { labelKey: MoneyPrepMessageKey; color: string; emoji: string }> = {
  lodging: { labelKey: 'ledger.category.lodging', color: 'var(--color-kind-lodging)', emoji: '🏨' },
  food: { labelKey: 'ledger.category.food', color: 'var(--color-kind-food)', emoji: '🍽️' },
  transport: { labelKey: 'ledger.category.transport', color: 'var(--color-kind-transit)', emoji: '🚃' },
  tickets: { labelKey: 'ledger.category.tickets', color: 'var(--color-kind-activity)', emoji: '🎟️' },
  other: { labelKey: 'ledger.category.other', color: 'var(--color-kind-other)', emoji: '🧾' },
};

export const CATEGORY_ORDER: readonly ExpenseCategory[] = ['lodging', 'food', 'transport', 'tickets', 'other'];

/** Linked stop kind to its most likely expense category. */
export const STOP_KIND_CATEGORY: Record<StopKind, ExpenseCategory> = {
  lodging: 'lodging',
  meal: 'food',
  transit: 'transport',
  activity: 'tickets',
  visit: 'other',
};

const CURRENCY_SYMBOL: Record<string, string> = { JPY: '¥', USD: '$', EUR: '€', GBP: '£' };
const FX_TO_USD: Record<string, number> = { JPY: 0.0066, USD: 1, EUR: 1.16, GBP: 1.34 };

export function currencySymbol(code: string): string {
  return CURRENCY_SYMBOL[code] ?? `${code} `;
}

/** Rate that converts an amount from `currency` into `base`. */
export function fxToBase(currency: string, base: string): number {
  return (FX_TO_USD[currency] ?? 1) / (FX_TO_USD[base] ?? 1);
}

export function money(amount: number, currency: string, locale: UiLocale = 'en'): string {
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency,
    currencyDisplay: 'narrowSymbol',
    maximumFractionDigits: currency === 'JPY' ? 0 : 2,
  }).format(amount);
}

export function moneyWhole(amount: number, currency: string, locale: UiLocale = 'en'): string {
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency,
    currencyDisplay: 'narrowSymbol',
    maximumFractionDigits: 0,
  }).format(amount);
}

/** Parse a non-negative custom share; empty input intentionally means zero. */
export function parseShare(value: string): number {
  if (value.trim() === '') return 0;
  const amount = Number(value);
  return Number.isFinite(amount) && amount >= 0 ? amount : NaN;
}

export function splitNote(note: string): { title: string; subtitle: string } {
  const separator = note.indexOf(' — ');
  if (separator < 0) return { title: note, subtitle: '' };
  return { title: note.slice(0, separator), subtitle: note.slice(separator + 3) };
}

export function splitParticipants(split: ExpenseSplit): string[] {
  return split.kind === 'even' ? split.participantIds : split.participants.map((participant) => participant.userId);
}

export function splitSummary(
  split: ExpenseSplit,
  amount: number,
  currency: string,
  locale: UiLocale,
  translate: LedgerTranslate,
): string {
  const participantCount = splitParticipants(split).length;
  if (split.kind === 'even') {
    return translate('ledger.split.evenSummary', {
      count: formatUiNumber(participantCount, locale),
      amount: money(amount / participantCount, currency, locale),
    });
  }
  return translate(participantCount === 1 ? 'ledger.split.customSummary.one' : 'ledger.split.customSummary.many', {
    count: formatUiNumber(participantCount, locale),
  });
}

export type SplitMode = 'even_all' | 'even_some' | 'custom';

export interface SplitStatus {
  badIds: string[];
  remainder: number;
  valid: boolean;
  blocker: string | null;
}

function toMinorUnit(amount: number, currency: string): number {
  const scale = currency === 'JPY' ? 1 : 100;
  return Math.round(amount * scale) / scale;
}

/** Single source of truth for whether the current split can be saved. */
export function splitStatus(
  mode: SplitMode,
  members: User[],
  selected: Set<string>,
  exact: Record<string, string>,
  amount: number,
  currency: string,
  locale: UiLocale,
  translate: LedgerTranslate,
): SplitStatus {
  if (mode !== 'custom') {
    const participantCount =
      mode === 'even_all' ? members.length : members.filter((member) => selected.has(member.id)).length;
    return {
      badIds: [],
      remainder: 0,
      valid: participantCount > 0,
      blocker: participantCount > 0 ? null : translate('ledger.split.pickOne'),
    };
  }

  const badIds = members
    .filter((member) => Number.isNaN(parseShare(exact[member.id] ?? '')))
    .map((member) => member.id);
  const total = members.reduce((sum, member) => sum + parseShare(exact[member.id] ?? ''), 0);
  const remainder = Number.isNaN(total) ? NaN : toMinorUnit(amount - total, currency);
  const assignedCount = members.filter((member) => parseShare(exact[member.id] ?? '') > 0).length;

  if (badIds.length > 0) return { badIds, remainder, valid: false, blocker: translate('ledger.split.invalidShare') };
  if (assignedCount === 0) return { badIds, remainder, valid: false, blocker: translate('ledger.split.assignOne') };
  if (remainder !== 0) {
    return {
      badIds,
      remainder,
      valid: false,
      blocker:
        remainder > 0
          ? translate('ledger.split.unassigned', {
              remainder: money(remainder, currency, locale),
              total: money(amount, currency, locale),
            })
          : translate('ledger.split.exceeds', { amount: money(-remainder, currency, locale) }),
    };
  }
  return { badIds, remainder, valid: true, blocker: null };
}

/** Build the API split shape for a validated split-control state. */
export function buildSplit(
  mode: SplitMode,
  members: User[],
  selected: Set<string>,
  exact: Record<string, string>,
  amount: number,
  currency: string,
): { split: ExpenseSplit; valid: boolean } {
  if (mode === 'even_all') {
    return { split: { kind: 'even', participantIds: members.map((member) => member.id) }, valid: members.length > 0 };
  }
  if (mode === 'even_some') {
    const participantIds = members.filter((member) => selected.has(member.id)).map((member) => member.id);
    return { split: { kind: 'even', participantIds }, valid: participantIds.length > 0 };
  }

  const participants = members
    .map((member) => ({ userId: member.id, amount: parseShare(exact[member.id] ?? '') }))
    .filter((participant) => participant.amount > 0);
  const total = members.reduce((sum, member) => sum + parseShare(exact[member.id] ?? ''), 0);
  return {
    split: { kind: 'exact', participants },
    valid:
      members.every((member) => !Number.isNaN(parseShare(exact[member.id] ?? ''))) &&
      participants.length > 0 &&
      toMinorUnit(amount - total, currency) === 0,
  };
}

export interface AddExpenseSeed {
  amount?: string;
  currency?: string;
  category?: ExpenseCategory;
  linkedStopId?: string;
  note?: string;
  splitMode?: SplitMode;
  exact?: Record<string, string>;
}
