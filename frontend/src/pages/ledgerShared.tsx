import { useMemo, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import type { Expense, ExpenseCategory, ExpenseSplit, User } from '../api/types';
import { fillStyle } from '../lib/oklch';
import { formatUiNumber, useI18n } from '../i18n';
import type { UiLocale } from '../i18n';
import type { MoneyPrepMessageKey } from '../i18n/messages.moneyPrep';

/** A linked-stop option in the add-expense composer's dropdown. */
export interface StopOption {
  id: string;
  label: string;
  stopKind: string;
  note: string;
}

/** A suggested min-cash-flow transfer (mirrors LedgerView.suggestedTransfers). */
export interface Transfer {
  fromUser: string;
  toUser: string;
  amount: number;
}

/**
 * Shared ledger vocabulary + the split control, add-expense modal and settle-up
 * surfaces (milestone 4). The Ledger page composes these; keeping them here
 * keeps LedgerTab focused on layout.
 */

export const CATEGORY_META: Record<ExpenseCategory, { labelKey: MoneyPrepMessageKey; color: string; emoji: string }> = {
  lodging: { labelKey: 'ledger.category.lodging', color: 'var(--color-kind-lodging)', emoji: '🏨' },
  food: { labelKey: 'ledger.category.food', color: 'var(--color-kind-food)', emoji: '🍽️' },
  transport: { labelKey: 'ledger.category.transport', color: 'var(--color-kind-transit)', emoji: '🚃' },
  tickets: { labelKey: 'ledger.category.tickets', color: 'var(--color-kind-activity)', emoji: '🎟️' },
  // `other` used to share --color-kind-transit with `transport`, so the two
  // categories were literally the same swatch in the filter bar, the category
  // picker and the expense icons — the colour encoded nothing. Its own token.
  other: { labelKey: 'ledger.category.other', color: 'var(--color-kind-other)', emoji: '🧾' },
};

export const CATEGORY_ORDER: ExpenseCategory[] = ['lodging', 'food', 'transport', 'tickets', 'other'];

/** Kind of a linked stop → a plausible default expense category. */
export const STOP_KIND_CATEGORY: Record<string, ExpenseCategory> = {
  lodging: 'lodging',
  meal: 'food',
  transit: 'transport',
  activity: 'tickets',
  visit: 'other',
};

const CURRENCY_SYMBOL: Record<string, string> = { JPY: '¥', USD: '$', EUR: '€', GBP: '£' };
/** To-USD rates (frozen); trip base is USD in the fixture. */
const FX_TO_USD: Record<string, number> = { JPY: 0.0066, USD: 1, EUR: 1.16, GBP: 1.34 };

export function currencySymbol(code: string): string {
  return CURRENCY_SYMBOL[code] ?? code + ' ';
}

/** Rate that multiplies an `amount` in `currency` into the trip `base`. */
export function fxToBase(currency: string, base: string): number {
  return (FX_TO_USD[currency] ?? 1) / (FX_TO_USD[base] ?? 1);
}

/** Natural formatting — JPY has no minor unit, everything else shows cents.
    `narrowSymbol` keeps it to ¥ / $ / € rather than the JP¥ / US$ long forms. */
export function money(amount: number, currency: string, locale: UiLocale = 'en'): string {
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency,
    currencyDisplay: 'narrowSymbol',
    maximumFractionDigits: currency === 'JPY' ? 0 : 2,
  }).format(amount);
}

/** Whole-unit formatting for balances / transfers (the bars read cleaner). */
export function moneyWhole(amount: number, currency: string, locale: UiLocale = 'en'): string {
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency,
    currencyDisplay: 'narrowSymbol',
    maximumFractionDigits: 0,
  }).format(amount);
}

/**
 * One custom-split field → a number, or NaN when the text isn't one.
 *
 * The old readers were `(v.trim() === '' ? 0 : Number(v)) || 0`, which folded
 * three different states into 0: empty (fine — that person is out of the
 * split), "abc" (nonsense) and "-40" (a negative share). So typing "abc" in a
 * ¥27,500 split left the remainder line reading "¥27,500 still unassigned" as
 * if the field were blank, and a negative share *increased* everyone else's
 * apparent room. Empty stays 0; anything unparseable or negative is NaN, which
 * poisons every total it touches and so can never round-trip to a saved split.
 */
export function parseShare(v: string): number {
  if (v.trim() === '') return 0;
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? n : NaN;
}

/** Split a "Title — description" note into its two display parts. */
export function splitNote(note: string): { title: string; subtitle: string } {
  const i = note.indexOf(' — ');
  if (i >= 0) return { title: note.slice(0, i), subtitle: note.slice(i + 3) };
  return { title: note, subtitle: '' };
}

/** The userIds an expense's split touches, in a stable order. */
export function splitParticipants(split: ExpenseSplit): string[] {
  if (split.kind === 'even') return split.participantIds;
  return split.participants.map((p) => p.userId);
}

/** One-line "split N ways · ¥X pp" (even) or "custom split · N people". */
export function splitSummary(
  split: ExpenseSplit,
  amount: number,
  currency: string,
  locale: UiLocale,
  ui: ReturnType<typeof useI18n>['t'],
): string {
  const n = splitParticipants(split).length;
  if (split.kind === 'even')
    return ui('ledger.split.evenSummary', {
      count: formatUiNumber(n, locale),
      amount: money(amount / n, currency, locale),
    });
  return ui(n === 1 ? 'ledger.split.customSummary.one' : 'ledger.split.customSummary.many', {
    count: formatUiNumber(n, locale),
  });
}

// ─────────────────────────── Split control ───────────────────────────

export type SplitMode = 'even_all' | 'even_some' | 'custom';

/** What the split control currently is, and — if it can't be saved — why. */
export interface SplitStatus {
  /** userIds whose field holds something that isn't a non-negative number. */
  badIds: string[];
  /** amount − Σ shares, to the currency's minor unit. NaN if any field is bad. */
  remainder: number;
  valid: boolean;
  /** The one sentence to put next to the disabled save button; null when valid. */
  blocker: string | null;
}

/** Round to the currency's smallest unit — ¥1, or a cent everywhere else. */
function toMinorUnit(n: number, currency: string): number {
  const unit = currency === 'JPY' ? 1 : 100;
  return Math.round(n * unit) / unit;
}

/**
 * Single source of truth for "can this split be saved, and if not, what do we
 * tell the user". Both the control (which renders the remainder inline) and the
 * modal footer (which repeats the blocker next to the greyed-out CTA, because
 * on a phone the inline line is a scroll away) read this.
 */
export function splitStatus(
  mode: SplitMode,
  members: User[],
  selected: Set<string>,
  exact: Record<string, string>,
  amount: number,
  currency: string,
  locale: UiLocale,
  ui: ReturnType<typeof useI18n>['t'],
): SplitStatus {
  if (mode !== 'custom') {
    const n = mode === 'even_all' ? members.length : members.filter((m) => selected.has(m.id)).length;
    return {
      badIds: [],
      remainder: 0,
      valid: n > 0,
      blocker: n > 0 ? null : ui('ledger.split.pickOne'),
    };
  }
  const badIds = members.filter((m) => Number.isNaN(parseShare(exact[m.id] ?? ''))).map((m) => m.id);
  const total = members.reduce((s, m) => s + parseShare(exact[m.id] ?? ''), 0);
  const remainder = Number.isNaN(total) ? NaN : toMinorUnit(amount - total, currency);
  const assigned = members.filter((m) => parseShare(exact[m.id] ?? '') > 0).length;
  if (badIds.length > 0) {
    return {
      badIds,
      remainder,
      valid: false,
      blocker: ui('ledger.split.invalidShare'),
    };
  }
  if (assigned === 0)
    return {
      badIds,
      remainder,
      valid: false,
      blocker: ui('ledger.split.assignOne'),
    };
  if (remainder !== 0) {
    return {
      badIds,
      remainder,
      valid: false,
      blocker:
        remainder > 0
          ? ui('ledger.split.unassigned', {
              remainder: money(remainder, currency, locale),
              total: money(amount, currency, locale),
            })
          : ui('ledger.split.exceeds', { amount: money(-remainder, currency, locale) }),
    };
  }
  return { badIds, remainder, valid: true, blocker: null };
}

/** Live, validated split editor — the three ExpenseSplit shapes with a
    remainder line that must reach 0 (custom) before the caller can save. */
export function SplitControl({
  members,
  amount,
  currency,
  payerId,
  mode,
  onModeChange,
  selected,
  onSelectedChange,
  exact,
  onExactChange,
}: {
  members: User[];
  amount: number;
  currency: string;
  payerId: string;
  mode: SplitMode;
  onModeChange: (m: SplitMode) => void;
  selected: Set<string>;
  onSelectedChange: (s: Set<string>) => void;
  exact: Record<string, string>;
  onExactChange: (e: Record<string, string>) => void;
}) {
  const { locale, t: ui, formatNumber } = useI18n();
  const isJpy = currency === 'JPY';
  const step = isJpy ? 1 : 0.01;

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    if (next.size === 0) return; // never empty — even needs ≥1
    onSelectedChange(next);
  };

  const evenIds = mode === 'even_all' ? members.map((m) => m.id) : [...selected];
  const perHead = evenIds.length ? amount / evenIds.length : 0;

  const status = splitStatus(mode, members, selected, exact, amount, currency, locale, ui);
  const remainder = status.remainder;
  const badField = new Set(status.badIds);

  const splitEvenly = () => {
    const per = Math.floor(amount / members.length / step) * step;
    const rounded = isJpy ? Math.floor(amount / members.length) : Math.round((amount / members.length) * 100) / 100;
    const next: Record<string, string> = {};
    let assigned = 0;
    members.forEach((m) => {
      next[m.id] = String(rounded);
      assigned += rounded;
    });
    // Absorb the rounding remainder onto the payer so the totals stay exact.
    const leftover = Math.round((amount - assigned) * (isJpy ? 1 : 100)) / (isJpy ? 1 : 100);
    const payer = payerId && next[payerId] !== undefined ? payerId : members[0].id;
    next[payer] = String(Math.round((rounded + leftover) * (isJpy ? 1 : 100)) / (isJpy ? 1 : 100));
    onExactChange(next);
    void per;
  };

  return (
    <div className="split-ctl">
      <div className="split-modes">
        <button
          type="button"
          className={mode === 'even_all' ? 'on' : ''}
          aria-pressed={mode === 'even_all'}
          onClick={() => onModeChange('even_all')}
        >
          {ui('ledger.split.mode.everyone')}
        </button>
        <button
          type="button"
          className={mode === 'even_some' ? 'on' : ''}
          aria-pressed={mode === 'even_some'}
          onClick={() => onModeChange('even_some')}
        >
          {ui('ledger.split.mode.some')}
        </button>
        <button
          type="button"
          className={mode === 'custom' ? 'on' : ''}
          aria-pressed={mode === 'custom'}
          onClick={() => onModeChange('custom')}
        >
          {ui('ledger.split.mode.custom', { symbol: currencySymbol(currency) })}
        </button>
      </div>
      <div className="split-body">
        {mode !== 'custom' ? (
          <>
            <div className="per-head">
              {mode === 'even_all' ? (
                <>
                  {/* A brand-new trip has one member, and this read "all 1
                      travellers" — same plural bug as the trip hero's. */}
                  {ui('ledger.split.evenAcross', {
                    travellers:
                      members.length === 1
                        ? ui('ledger.split.onlyTraveller')
                        : ui('ledger.split.allTravellers', { count: formatNumber(members.length) }),
                    amount: money(perHead, currency, locale),
                  })}
                </>
              ) : (
                <>
                  {ui('ledger.split.selected', {
                    total: money(amount, currency, locale),
                    count: formatNumber(evenIds.length),
                    amount: money(perHead || 0, currency, locale),
                  })}
                </>
              )}
            </div>
            <div className="split-chips">
              {members.map((m) => {
                const on = mode === 'even_all' || selected.has(m.id);
                return (
                  <button
                    key={m.id}
                    type="button"
                    className={`split-chip${on ? ' on' : ''}`}
                    aria-pressed={on}
                    onClick={() => mode === 'even_some' && toggle(m.id)}
                    style={mode === 'even_all' ? { cursor: 'default' } : undefined}
                  >
                    <span className="ck">{on ? '✓' : ''}</span>
                    <span className="avatar xs" style={fillStyle(m.avatarColor)}>
                      {m.displayName[0]}
                    </span>
                    {m.displayName}
                  </button>
                );
              })}
            </div>
          </>
        ) : (
          <>
            <div className="per-head">
              {ui('ledger.split.enterShares', { amount: money(amount, currency, locale) })}
            </div>
            {members.map((m) => {
              // Red means "*this* field is wrong", not "the column doesn't add
              // up" — the old rule reddened every filled field whenever the
              // remainder was non-zero, so a perfectly good ¥5,500 looked like
              // the error while the actual "abc" two rows down looked fine.
              const bad = badField.has(m.id);
              return (
                <div key={m.id} className="split-mem">
                  <span className="who">
                    <span className="avatar xs" style={fillStyle(m.avatarColor)}>
                      {m.displayName[0]}
                    </span>
                    {m.displayName}
                  </span>
                  <span className="exact-in">
                    <span className="pfx">{currencySymbol(currency)}</span>
                    <input
                      inputMode="decimal"
                      className={bad ? 'bad' : ''}
                      aria-invalid={bad || undefined}
                      aria-label={`${m.displayName}${ui('ledger.split.shareAriaSuffix')}`}
                      value={exact[m.id] ?? ''}
                      onChange={(e) => onExactChange({ ...exact, [m.id]: e.target.value })}
                      placeholder="0"
                    />
                  </span>
                </div>
              );
            })}
            {/* Prose left, figure right. Both halves used to print the same
                number ("¥8,600 still unassigned … ¥8,600 left"), which made the
                right column look like a second, different quantity. */}
            <div className={`remainder ${remainder === 0 ? 'ok' : 'bad'}`} role="status">
              <span>
                {Number.isNaN(remainder)
                  ? `⚠ ${ui('ledger.split.remainder.invalid')}`
                  : remainder === 0
                    ? `✓ ${ui('ledger.split.remainder.complete')}`
                    : remainder > 0
                      ? `⚠ ${ui('ledger.split.remainder.unassigned')}`
                      : `⚠ ${ui('ledger.split.remainder.over')}`}
              </span>
              <span className="n">
                {Number.isNaN(remainder)
                  ? '—'
                  : remainder === 0
                    ? money(amount, currency, locale)
                    : ui(remainder > 0 ? 'ledger.split.remainder.left' : 'ledger.split.remainder.overAmount', {
                        amount: money(Math.abs(remainder), currency, locale),
                      })}
              </span>
            </div>
            <button type="button" className="split-evenly" onClick={splitEvenly}>
              {ui('ledger.split.evenly')}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

/** Build the ExpenseSplit for the current control state (or null if invalid). */
export function buildSplit(
  mode: SplitMode,
  members: User[],
  selected: Set<string>,
  exact: Record<string, string>,
  amount: number,
  currency: string,
): { split: ExpenseSplit; valid: boolean } {
  if (mode === 'even_all') {
    return { split: { kind: 'even', participantIds: members.map((m) => m.id) }, valid: members.length > 0 };
  }
  if (mode === 'even_some') {
    const ids = members.filter((m) => selected.has(m.id)).map((m) => m.id);
    return { split: { kind: 'even', participantIds: ids }, valid: ids.length > 0 };
  }
  // `parseShare` returns NaN for junk, so a bad field can never be silently
  // filtered out here as a 0 and let a wrong-but-balanced split through.
  const participants = members
    .map((m) => ({ userId: m.id, amount: parseShare(exact[m.id] ?? '') }))
    .filter((p) => p.amount > 0);
  return {
    split: { kind: 'exact', participants },
    valid:
      members.every((m) => !Number.isNaN(parseShare(exact[m.id] ?? ''))) &&
      participants.length > 0 &&
      toMinorUnit(amount - members.reduce((sum, member) => sum + parseShare(exact[member.id] ?? ''), 0), currency) ===
        0,
  };
}

/** Stamp avatars for a set of userIds (split heads, coverage). */
export function Heads({ ids, membersById, meId }: { ids: string[]; membersById: Map<string, User>; meId?: string }) {
  return (
    <span className="heads">
      {ids.map((id) => {
        const u = membersById.get(id);
        if (!u) return null;
        return (
          <span
            key={id}
            className={`avatar xs${id === meId ? ' me' : ''}`}
            style={fillStyle(u.avatarColor)}
            title={u.displayName}
          >
            {u.displayName[0]}
          </span>
        );
      })}
    </span>
  );
}

// ─────────────────────────── Add-expense modal ───────────────────────────

export interface AddExpenseSeed {
  amount?: string;
  currency?: string;
  category?: ExpenseCategory;
  linkedStopId?: string;
  note?: string;
  splitMode?: SplitMode;
  exact?: Record<string, string>;
}

/**
 * Currencies always offered alongside the trip's own base. The trip base is
 * prepended and the list deduped, so a GBP trip gets GBP first and no dupe.
 */
const COMMON_CURRENCIES = ['JPY', 'USD', 'EUR', 'GBP'];

/** The existing split, back into control state. */
function seedFromExpense(
  e: Expense,
  members: User[],
): { mode: SplitMode; selected: Set<string>; exact: Record<string, string> } {
  if (e.split.kind === 'exact') {
    const exact: Record<string, string> = {};
    for (const p of e.split.participants) exact[p.userId] = String(p.amount);
    return { mode: 'custom', selected: new Set(e.split.participants.map((p) => p.userId)), exact };
  }
  const ids = splitParticipants(e.split);
  // An even split covering the whole group is "Even · everyone", not "some of
  // us" that happens to have everyone ticked — the two tabs mean different
  // things when someone is later added to the trip.
  const everyone = members.length > 0 && members.every((m) => ids.includes(m.id));
  return { mode: everyone ? 'even_all' : 'even_some', selected: new Set(ids), exact: {} };
}

/**
 * The flagship add-expense flow (§ mockup B/C). Payer, amount + currency,
 * category, an optional linked stop that auto-seeds category & note, and the
 * live split control. Writes straight through `addExpense` — expenses are
 * records, not gated plan edits.
 *
 * Pass `expense` to open the same surface in edit mode: same fields, same
 * validation, `updateExpense` instead of `addExpense`, plus a delete. The
 * composer is the detail view — there is nothing about an expense the composer
 * doesn't already show, so a separate read-only sheet would just be a second
 * layout to keep in sync.
 */
export function AddExpenseModal({
  members,
  meId,
  tripId,
  base,
  stops,
  onClose,
  onAdded,
  seed,
  expense,
}: {
  members: User[];
  meId: string;
  tripId: string;
  base: string;
  stops: StopOption[];
  onClose: () => void;
  onAdded: (expenseId: string) => void;
  seed?: AddExpenseSeed;
  expense?: Expense;
}) {
  const api = useApi();
  const { locale, t: ui } = useI18n();
  const queryClient = useQueryClient();
  const editing = !!expense;
  const fromExpense = expense ? seedFromExpense(expense, members) : null;

  const [payerId, setPayerId] = useState(expense?.paidBy ?? meId);
  const [amountStr, setAmountStr] = useState(expense ? String(expense.amount) : (seed?.amount ?? ''));
  // Defaulted to a hardcoded 'JPY', so a EUR trip opened its composer showing
  // ¥ and would have logged a euro dinner as yen at 0.0066×. The trip's own
  // base is the only defensible default.
  const [currency, setCurrency] = useState(expense?.currency ?? seed?.currency ?? base);
  const [category, setCategory] = useState<ExpenseCategory>(expense?.category ?? seed?.category ?? 'food');
  const [linkedStopId, setLinkedStopId] = useState(expense?.linkedStopId ?? seed?.linkedStopId ?? '');
  const [note, setNote] = useState(expense?.note ?? seed?.note ?? '');
  const [justLinked, setJustLinked] = useState(!expense && !!seed?.linkedStopId);

  const [mode, setMode] = useState<SplitMode>(fromExpense?.mode ?? seed?.splitMode ?? 'even_all');
  const [selected, setSelected] = useState<Set<string>>(fromExpense?.selected ?? new Set(members.map((m) => m.id)));
  const [exact, setExact] = useState<Record<string, string>>(fromExpense?.exact ?? seed?.exact ?? {});
  const [confirmDelete, setConfirmDelete] = useState(false);

  // The base first, then the rest of the usual suspects, deduped — the segment
  // was a hardcoded ['JPY','USD','EUR'], so on a GBP trip there was no way to
  // record an expense in the trip's own base currency at all.
  const currencies = [base, ...COMMON_CURRENCIES.filter((c) => c !== base)];

  const amount = amountStr.trim() === '' ? 0 : Number(amountStr) || 0;
  const toBase = fxToBase(currency, base);
  const { split } = buildSplit(mode, members, selected, exact, amount, currency);
  const status = splitStatus(mode, members, selected, exact, amount, currency, locale, ui);
  const canSave = amount > 0 && status.valid;
  // The one line the footer shows beside a disabled CTA. On a phone the split's
  // own remainder line is often below the fold while the footer is pinned in
  // view, so "Add ¥8,600" greyed out with no reason was the whole story.
  const blocker =
    amountStr.trim() === ''
      ? ui('ledger.expense.enterAmount')
      : amount <= 0
        ? ui('ledger.expense.positiveAmount')
        : status.blocker;

  const onLinkStop = (id: string) => {
    setLinkedStopId(id);
    const stop = stops.find((s) => s.id === id);
    if (stop) {
      setCategory(STOP_KIND_CATEGORY[stop.stopKind] ?? category);
      if (stop.note) setNote(stop.note);
      setJustLinked(true);
    } else {
      setJustLinked(false);
    }
  };

  const add = useMutation({
    mutationFn: () =>
      api.addExpense(tripId, {
        paidBy: payerId,
        amount,
        currency,
        category,
        split,
        note: note.trim(),
        linkedStopId: linkedStopId || undefined,
      }),
    onSuccess: (added) => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      onAdded(added.id);
      onClose();
    },
  });

  const save = useMutation({
    mutationFn: () =>
      api.updateExpense(expense!.id, {
        paidBy: payerId,
        amount,
        currency,
        category,
        split,
        note: note.trim(),
        linkedStopId: linkedStopId || null,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      onClose();
    },
  });

  const remove = useMutation({
    mutationFn: () => api.deleteExpense(expense!.id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      onClose();
    },
  });

  const busy = add.isPending || save.isPending || remove.isPending;
  const meta = CATEGORY_META[category];

  return (
    <SheetModal onClose={onClose}>
      <div
        className="exp-modal"
        role="dialog"
        aria-modal="true"
        aria-label={ui(editing ? 'ledger.expense.editTitle' : 'ledger.expense.addTitle')}
      >
        <div className="mtop">
          <span className="mtop-ic" style={{ background: meta.color }}>
            {meta.emoji}
          </span>
          <strong>{ui(editing ? 'ledger.expense.editTitle' : 'ledger.expense.addTitle')}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={ui('ledger.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <span className="fl">{ui('ledger.expense.whoPaid')}</span>
            <span className="fv">
              <span className="mem-pick">
                {members.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    className={`mem-opt${m.id === payerId ? ' sel payer' : ''}`}
                    aria-pressed={m.id === payerId}
                    onClick={() => setPayerId(m.id)}
                  >
                    <span className="avatar xs" style={fillStyle(m.avatarColor)}>
                      {m.displayName[0]}
                    </span>
                    {m.displayName}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('ledger.expense.amount')}</span>
            <span className="fv">
              <span className="amount-box">
                <span className="cur">{currencySymbol(currency)}</span>
                <input
                  inputMode="decimal"
                  value={amountStr}
                  onChange={(e) => setAmountStr(e.target.value)}
                  placeholder="0"
                  aria-label={ui('ledger.expense.amount')}
                />
              </span>
              <span className="cur-seg">
                {currencies.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={c === currency ? 'on' : ''}
                    aria-pressed={c === currency}
                    onClick={() => setCurrency(c)}
                  >
                    {c}
                  </button>
                ))}
              </span>
            </span>
          </div>
          {currency !== base && amount > 0 && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                {/* Said "FX frozen … (fxRateToBase)" — the storage field name,
                    which means nothing to anyone who hasn't read the schema. */}
                <span className="fx-hint">
                  ≈ <b>{money(amount * toBase, base, locale)}</b> {ui('ledger.expense.fxPrefix')}{' '}
                  {currencySymbol(currency)}
                  {new Intl.NumberFormat(locale, {
                    minimumFractionDigits: currency === 'JPY' ? 1 : 2,
                    maximumFractionDigits: currency === 'JPY' ? 1 : 2,
                  }).format(1 / toBase)}
                  /{currencySymbol(base)} — {editing ? ui('ledger.expense.fxEdit') : ui('ledger.expense.fxNew')}
                </span>
              </span>
            </div>
          )}

          <div className="frow">
            <span className="fl">{ui('ledger.expense.category')}</span>
            <span className="fv">
              <span className="cat-pick">
                {CATEGORY_ORDER.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={`cat-opt${c === category ? ' sel' : ''}`}
                    aria-pressed={c === category}
                    onClick={() => setCategory(c)}
                  >
                    <span className="kd" style={{ background: CATEGORY_META[c].color }} />
                    {ui(CATEGORY_META[c].labelKey)}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('ledger.expense.linkStop')}</span>
            <span className="fv col">
              <select
                className="tinp"
                value={linkedStopId}
                aria-label={ui('ledger.expense.linkStop')}
                onChange={(e) => onLinkStop(e.target.value)}
              >
                <option value="">{ui('ledger.expense.noLinkedStop')}</option>
                {stops.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
              {justLinked && linkedStopId && (
                <span className="link-suggest">
                  ✓ {ui('ledger.expense.autoFilledPrefix')} <b>{ui(meta.labelKey)}</b>{' '}
                  {ui('ledger.expense.autoFilledSuffix')}
                </span>
              )}
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('ledger.expense.note')}</span>
            <span className="fv">
              <input
                className="tinp"
                value={note}
                aria-label={ui('ledger.expense.note')}
                onChange={(e) => {
                  setNote(e.target.value);
                  setJustLinked(false);
                }}
                placeholder={ui('ledger.expense.notePlaceholder')}
              />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">{ui('ledger.expense.split')}</span>
            <span className="fv col">
              <SplitControl
                members={members}
                amount={amount}
                currency={currency}
                payerId={payerId}
                mode={mode}
                onModeChange={setMode}
                selected={selected}
                onSelectedChange={setSelected}
                exact={exact}
                onExactChange={setExact}
              />
            </span>
          </div>
        </div>
        <div className="exp-foot">
          {/* The blocker replaces the standing hint rather than sitting beside
              it: a disabled button with an unrelated sentence next to it reads
              as "this is just how it is", not as "here is what's missing". */}
          {blocker ? (
            <span className="foot-blocker grow" role="status">
              <span aria-hidden>⚠</span> {blocker}
            </span>
          ) : (
            <span className="hint grow">
              {editing ? (
                <>
                  {ui('ledger.expense.editHint')} <b>{ui('ledger.expense.noApproval')}</b>
                </>
              ) : (
                <>
                  {ui('ledger.expense.addHint')} <b>{ui('ledger.expense.recordsHint')}</b>
                </>
              )}
            </span>
          )}
          {editing &&
            (confirmDelete ? (
              <button type="button" className="btn danger" disabled={busy} onClick={() => remove.mutate()}>
                {ui('ledger.expense.deleteForever')}
              </button>
            ) : (
              <button type="button" className="btn" disabled={busy} onClick={() => setConfirmDelete(true)}>
                {ui('ledger.expense.delete')}
              </button>
            ))}
          <button type="button" className="btn" onClick={onClose}>
            {ui('ledger.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || busy}
            onClick={() => (editing ? save.mutate() : add.mutate())}
          >
            {editing
              ? ui('ledger.expense.save')
              : amount > 0
                ? ui('ledger.expense.addAmount', { amount: money(amount, currency, locale) })
                : ui('ledger.addExpense')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}

// ─────────────────────────── Settle up ───────────────────────────

/**
 * Settle-up view (§ mockup D). The min-cash-flow suggestions, only the current
 * user's own outgoing transfer emphasised. Recording one appends a settlement
 * and re-balances everyone; when nothing's left it collapses to "All square".
 */
export function SettleUpModal({
  transfers,
  members,
  meId,
  tripId,
  base,
  onClose,
  initialConfirm,
}: {
  transfers: Transfer[];
  members: User[];
  meId: string;
  tripId: string;
  base: string;
  onClose: () => void;
  initialConfirm?: boolean;
}) {
  const api = useApi();
  const { locale, t: ui, formatNumber } = useI18n();
  const queryClient = useQueryClient();
  const byId = useMemo(() => new Map(members.map((m) => [m.id, m])), [members]);
  const name = (id: string) => byId.get(id)?.displayName ?? id;
  const mineFirst = initialConfirm ? (transfers.find((t) => t.fromUser === meId) ?? null) : null;

  const [confirming, setConfirming] = useState<Transfer | null>(mineFirst);
  const [amountStr, setAmountStr] = useState(mineFirst ? String(mineFirst.amount) : '');

  const startConfirm = (t: Transfer) => {
    setConfirming(t);
    setAmountStr(String(t.amount));
  };

  /**
   * The amount actually being recorded, straight from the field.
   *
   * It used to be `Number(amountStr) || t.amount`, which is three bugs in one
   * expression: clearing the field or typing "abc" silently recorded the *full*
   * suggested transfer (the `||` fallback), and "-50" recorded a negative
   * settlement — which the ledger applies as a transfer the other way, pushing
   * both balances further apart instead of toward zero. No fallback now: what
   * the field says is what gets written, and if it isn't a positive number
   * nothing gets written at all.
   */
  const entered = amountStr.trim() === '' ? NaN : Number(amountStr);
  const validAmount = Number.isFinite(entered) && entered > 0;
  const overSuggested = validAmount && confirming ? entered > confirming.amount : false;

  const record = useMutation({
    mutationFn: (t: Transfer) => api.addSettlement(tripId, { fromUser: t.fromUser, toUser: t.toUser, amount: entered }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      setConfirming(null);
    },
  });

  const av = (id: string) => {
    const u = byId.get(id);
    return (
      <span className="avatar sm" style={fillStyle(u?.avatarColor ?? '#888')}>
        {u?.displayName[0] ?? '?'}
      </span>
    );
  };

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={ui('ledger.settleUp')}>
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--color-ok)' }}>
            🤝
          </span>
          <strong>{ui('ledger.settleUp')}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={ui('ledger.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          {confirming ? (
            <div className="confirm-card">
              <h4>{ui('ledger.settle.recordTitle')}</h4>
              <div className="confirm-row">
                {av(confirming.fromUser)}
                <b>{confirming.fromUser === meId ? ui('ledger.settle.you') : name(confirming.fromUser)}</b>
                <span className="ar">{ui('ledger.settle.paid')}</span>
                {av(confirming.toUser)}
                <b>{name(confirming.toUser)}</b>
                {/* Drive the summary from the field, not from the suggestion.
                    With the field edited to 40 this row still read "$120" — the
                    confirmation restating a number the user had just changed. */}
                <span className="amt">{validAmount ? money(entered, base, locale) : '—'}</span>
              </div>
              <label className="hint confirm-amt">
                {ui('ledger.settle.amountEditable')}
                <span className="amount-box">
                  <span className="cur">{currencySymbol(base)}</span>
                  <input
                    inputMode="decimal"
                    value={amountStr}
                    onChange={(e) => setAmountStr(e.target.value)}
                    aria-label={ui('ledger.settle.amountIn', { currency: base })}
                    aria-invalid={!validAmount || undefined}
                    className={validAmount ? undefined : 'bad'}
                  />
                </span>
              </label>
              {!validAmount && (
                <p className="hint bad" role="status">
                  ⚠ {ui('ledger.settle.invalidAmountPrefix')} {currencySymbol(base)}0{' '}
                  {ui('ledger.settle.invalidAmountSuffix')}
                </p>
              )}
              {overSuggested && (
                <p className="hint warn" role="status">
                  ⚠ {ui('ledger.settle.overPrefix')} {money(confirming.amount, base, locale)}{' '}
                  {ui('ledger.settle.overSuggested')} {name(confirming.toUser)} {ui('ledger.settle.overSuffix')}
                </p>
              )}
              <div className="confirm-foot">
                <button type="button" className="btn sm" onClick={() => setConfirming(null)}>
                  {ui('ledger.cancel')}
                </button>
                <button
                  type="button"
                  className="btn accent sm"
                  disabled={record.isPending || !validAmount}
                  onClick={() => record.mutate(confirming)}
                >
                  {ui('ledger.settle.confirm')}
                </button>
              </div>
              <p className="hint">{ui('ledger.settle.writeHint', { currency: base })}</p>
            </div>
          ) : transfers.length === 0 ? (
            <div className="allsquare">
              <span className="em">🎉</span>
              <strong>{ui('ledger.settle.allSquare')}</strong>
              <span className="muted">{ui('ledger.settle.noneLeft')}</span>
            </div>
          ) : (
            <>
              <div className="settle-head">
                <strong>
                  {ui(transfers.length === 1 ? 'ledger.settle.transfer.one' : 'ledger.settle.transfer.many', {
                    count: formatNumber(transfers.length),
                  })}
                </strong>
                <span className="hint">
                  {ui('ledger.settle.inBase')} <b>{base}</b>
                </span>
              </div>
              <div className="settle-list">
                {transfers.map((t, i) => {
                  const mine = t.fromUser === meId;
                  return (
                    <div key={i} className={`settle-sug${mine ? ' mine' : ''}`}>
                      {av(t.fromUser)}
                      <span className="flow">
                        <b>{mine ? ui('ledger.settle.you') : name(t.fromUser)}</b>
                        <span className="ar">→</span>
                        {av(t.toUser)}
                        <b>{name(t.toUser)}</b>
                      </span>
                      <span className="amt">{moneyWhole(t.amount, base, locale)}</span>
                      <button type="button" className="btn accent sm" onClick={() => startConfirm(t)}>
                        {ui('ledger.settle.record')}
                        {mine ? ' →' : ''}
                      </button>
                    </div>
                  );
                })}
              </div>
              <p className="hint">
                {ui('ledger.settle.groupHintPrefix')} <b>{ui('ledger.settle.groupHintOwn')}</b>{' '}
                {ui('ledger.settle.groupHintSuffix')}
              </p>
            </>
          )}
        </div>
      </div>
    </SheetModal>
  );
}
