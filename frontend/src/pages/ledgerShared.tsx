import { useMemo, useState } from 'react';
import type { CSSProperties } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import type { ExpenseCategory, ExpenseSplit, User } from '../api/types';

/** A linked-stop option in the add-expense composer's dropdown. */
export interface StopOption { id: string; label: string; stopKind: string; note: string; }

/** A suggested min-cash-flow transfer (mirrors LedgerView.suggestedTransfers). */
export interface Transfer { fromUser: string; toUser: string; amount: number; }

/**
 * Shared ledger vocabulary + the split control, add-expense modal and settle-up
 * surfaces (milestone 4). The Ledger page composes these; keeping them here
 * keeps LedgerTab focused on layout.
 */

export const CATEGORY_META: Record<ExpenseCategory, { label: string; color: string; emoji: string }> = {
  lodging: { label: 'Lodging', color: 'var(--color-kind-lodging)', emoji: '🏨' },
  food: { label: 'Food', color: 'var(--color-kind-food)', emoji: '🍽️' },
  transport: { label: 'Transport', color: 'var(--color-kind-transit)', emoji: '🚃' },
  tickets: { label: 'Tickets', color: 'var(--color-kind-activity)', emoji: '🎟️' },
  other: { label: 'Other', color: 'var(--color-kind-transit)', emoji: '🧾' },
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
export function money(amount: number, currency: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency,
    currencyDisplay: 'narrowSymbol',
    maximumFractionDigits: currency === 'JPY' ? 0 : 2,
  }).format(amount);
}

/** Whole-unit formatting for balances / transfers (the bars read cleaner). */
export function moneyWhole(amount: number, currency: string): string {
  return new Intl.NumberFormat(undefined, { style: 'currency', currency, currencyDisplay: 'narrowSymbol', maximumFractionDigits: 0 }).format(amount);
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
export function splitSummary(split: ExpenseSplit, amount: number, currency: string): string {
  const n = splitParticipants(split).length;
  if (split.kind === 'even') return `split ${n} ways · ${money(amount / n, currency)} pp`;
  return `custom split · ${n} ${n === 1 ? 'person' : 'people'}`;
}

// ─────────────────────────── Split control ───────────────────────────

export type SplitMode = 'even_all' | 'even_some' | 'custom';

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
  const isJpy = currency === 'JPY';
  const step = isJpy ? 1 : 0.01;
  const parse = (v: string) => (v.trim() === '' ? 0 : Number(v)) || 0;

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    if (next.size === 0) return; // never empty — even needs ≥1
    onSelectedChange(next);
  };

  const evenIds = mode === 'even_all' ? members.map((m) => m.id) : [...selected];
  const perHead = evenIds.length ? amount / evenIds.length : 0;

  const exactTotal = members.reduce((s, m) => s + parse(exact[m.id] ?? ''), 0);
  const remainder = Math.round((amount - exactTotal) * (isJpy ? 1 : 100)) / (isJpy ? 1 : 100);

  const splitEvenly = () => {
    const per = Math.floor((amount / members.length) / step) * step;
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
        <button type="button" className={mode === 'even_all' ? 'on' : ''} onClick={() => onModeChange('even_all')}>Even · everyone</button>
        <button type="button" className={mode === 'even_some' ? 'on' : ''} onClick={() => onModeChange('even_some')}>Even · some of us</button>
        <button type="button" className={mode === 'custom' ? 'on' : ''} onClick={() => onModeChange('custom')}>Custom {currencySymbol(currency)}</button>
      </div>
      <div className="split-body">
        {mode !== 'custom' ? (
          <>
            <div className="per-head">
              {mode === 'even_all' ? (
                <>Even across <b>all {members.length} travellers</b> — <b>{money(perHead, currency)} each</b>.</>
              ) : (
                <>Split <b>{money(amount, currency)}</b> across <b>{evenIds.length} selected</b> — <b>{money(perHead || 0, currency)} each</b>. Tap a chip to add/remove.</>
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
                    onClick={() => mode === 'even_some' && toggle(m.id)}
                    style={mode === 'even_all' ? { cursor: 'default' } : undefined}
                  >
                    <span className="ck">{on ? '✓' : ''}</span>
                    <span className="avatar xs" style={{ background: m.avatarColor }}>{m.displayName[0]}</span>
                    {m.displayName}
                  </button>
                );
              })}
            </div>
          </>
        ) : (
          <>
            <div className="per-head">Enter each person's share of <b>{money(amount, currency)}</b>.</div>
            {members.map((m) => {
              const bad = remainder !== 0 && parse(exact[m.id] ?? '') > 0;
              return (
                <div key={m.id} className="split-mem">
                  <span className="who">
                    <span className="avatar xs" style={{ background: m.avatarColor }}>{m.displayName[0]}</span>
                    {m.displayName}
                  </span>
                  <span className="exact-in">
                    <span className="pfx">{currencySymbol(currency)}</span>
                    <input
                      inputMode="decimal"
                      className={bad ? 'bad' : ''}
                      value={exact[m.id] ?? ''}
                      onChange={(e) => onExactChange({ ...exact, [m.id]: e.target.value })}
                      placeholder="0"
                    />
                  </span>
                </div>
              );
            })}
            <div className={`remainder ${remainder === 0 ? 'ok' : 'bad'}`}>
              <span>
                {remainder === 0
                  ? `✓ Adds up to ${money(amount, currency)}`
                  : remainder > 0
                    ? `⚠ ${money(remainder, currency)} still unassigned — allocate it to save`
                    : `⚠ Over by ${money(-remainder, currency)} — trim a share`}
              </span>
              <span className="n">{remainder === 0 ? `${currencySymbol(currency)}0 left` : `${money(Math.abs(remainder), currency)} ${remainder > 0 ? 'left' : 'over'}`}</span>
            </div>
            <button type="button" className="split-evenly" onClick={splitEvenly}>Split evenly — drop any rounding on the payer</button>
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
  const isJpy = currency === 'JPY';
  const parse = (v: string) => (v.trim() === '' ? 0 : Number(v)) || 0;
  const participants = members
    .map((m) => ({ userId: m.id, amount: parse(exact[m.id] ?? '') }))
    .filter((p) => p.amount > 0);
  const total = participants.reduce((s, p) => s + p.amount, 0);
  const remainder = Math.round((amount - total) * (isJpy ? 1 : 100)) / (isJpy ? 1 : 100);
  return { split: { kind: 'exact', participants }, valid: remainder === 0 && participants.length > 0 };
}

/** Stamp avatars for a set of userIds (split heads, coverage). */
export function Heads({ ids, membersById, meId }: { ids: string[]; membersById: Map<string, User>; meId?: string }) {
  return (
    <span className="heads">
      {ids.map((id) => {
        const u = membersById.get(id);
        if (!u) return null;
        return (
          <span key={id} className={`avatar xs${id === meId ? ' me' : ''}`} style={{ background: u.avatarColor } as CSSProperties} title={u.displayName}>
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
 * The flagship add-expense flow (§ mockup B/C). Payer, amount + currency,
 * category, an optional linked stop that auto-seeds category & note, and the
 * live split control. Writes straight through `addExpense` — expenses are
 * records, not gated plan edits.
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
}: {
  members: User[];
  meId: string;
  tripId: string;
  base: string;
  stops: StopOption[];
  onClose: () => void;
  onAdded: (expenseId: string) => void;
  seed?: AddExpenseSeed;
}) {
  const api = useApi();
  const queryClient = useQueryClient();

  const [payerId, setPayerId] = useState(meId);
  const [amountStr, setAmountStr] = useState(seed?.amount ?? '');
  const [currency, setCurrency] = useState(seed?.currency ?? 'JPY');
  const [category, setCategory] = useState<ExpenseCategory>(seed?.category ?? 'food');
  const [linkedStopId, setLinkedStopId] = useState(seed?.linkedStopId ?? '');
  const [note, setNote] = useState(seed?.note ?? '');
  const [justLinked, setJustLinked] = useState(!!seed?.linkedStopId);

  const [mode, setMode] = useState<SplitMode>(seed?.splitMode ?? 'even_all');
  const [selected, setSelected] = useState<Set<string>>(new Set(members.map((m) => m.id)));
  const [exact, setExact] = useState<Record<string, string>>(seed?.exact ?? {});

  const amount = amountStr.trim() === '' ? 0 : Number(amountStr) || 0;
  const toBase = fxToBase(currency, base);
  const { split, valid: splitValid } = buildSplit(mode, members, selected, exact, amount, currency);
  const canSave = amount > 0 && splitValid;

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
    onSuccess: (expense) => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      onAdded(expense.id);
      onClose();
    },
  });

  const meta = CATEGORY_META[category];

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label="Add an expense">
        <div className="mtop">
          <span className="mtop-ic" style={{ background: meta.color }}>{meta.emoji}</span>
          <strong>Add an expense</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <span className="fl">Who paid</span>
            <span className="fv">
              <span className="mem-pick">
                {members.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    className={`mem-opt${m.id === payerId ? ' sel payer' : ''}`}
                    onClick={() => setPayerId(m.id)}
                  >
                    <span className="avatar xs" style={{ background: m.avatarColor }}>{m.displayName[0]}</span>
                    {m.displayName}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">Amount</span>
            <span className="fv">
              <span className="amount-box">
                <span className="cur">{currencySymbol(currency)}</span>
                <input inputMode="decimal" value={amountStr} onChange={(e) => setAmountStr(e.target.value)} placeholder="0" aria-label="Amount" />
              </span>
              <span className="cur-seg">
                {['JPY', 'USD', 'EUR'].map((c) => (
                  <button key={c} type="button" className={c === currency ? 'on' : ''} onClick={() => setCurrency(c)}>{c}</button>
                ))}
              </span>
            </span>
          </div>
          {currency !== base && amount > 0 && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="fx-hint">
                  ≈ <b>{money(amount * toBase, base)}</b> at {currencySymbol(currency)}{(1 / toBase).toFixed(currency === 'JPY' ? 1 : 2)}/{currencySymbol(base)} — FX frozen the moment you save (fxRateToBase).
                </span>
              </span>
            </div>
          )}

          <div className="frow">
            <span className="fl">Category</span>
            <span className="fv">
              <span className="cat-pick">
                {CATEGORY_ORDER.map((c) => (
                  <button key={c} type="button" className={`cat-opt${c === category ? ' sel' : ''}`} onClick={() => setCategory(c)}>
                    <span className="kd" style={{ background: CATEGORY_META[c].color }} />
                    {CATEGORY_META[c].label}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">Link a stop</span>
            <span className="fv col">
              <select className="tinp" value={linkedStopId} onChange={(e) => onLinkStop(e.target.value)}>
                <option value="">— no linked stop —</option>
                {stops.map((s) => (
                  <option key={s.id} value={s.id}>{s.label}</option>
                ))}
              </select>
              {justLinked && linkedStopId && (
                <span className="link-suggest">✓ Auto-filled <b>category: {meta.label}</b> and the note from the stop — edit either if you like.</span>
              )}
            </span>
          </div>

          <div className="frow">
            <span className="fl">Note</span>
            <span className="fv">
              <input className="tinp" value={note} onChange={(e) => { setNote(e.target.value); setJustLinked(false); }} placeholder="What was it for?" />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Split</span>
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
          <span className="hint grow">Expenses apply immediately — no approval. <b>Records, not plan edits.</b></span>
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="button" className="btn accent" disabled={!canSave || add.isPending} onClick={() => add.mutate()}>
            Add {amount > 0 ? money(amount, currency) : 'expense'}
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
  const queryClient = useQueryClient();
  const byId = useMemo(() => new Map(members.map((m) => [m.id, m])), [members]);
  const name = (id: string) => byId.get(id)?.displayName ?? id;
  const mineFirst = initialConfirm ? transfers.find((t) => t.fromUser === meId) ?? null : null;

  const [confirming, setConfirming] = useState<Transfer | null>(mineFirst);
  const [amountStr, setAmountStr] = useState(mineFirst ? String(mineFirst.amount) : '');

  const startConfirm = (t: Transfer) => {
    setConfirming(t);
    setAmountStr(String(t.amount));
  };

  const record = useMutation({
    mutationFn: (t: Transfer) =>
      api.addSettlement(tripId, { fromUser: t.fromUser, toUser: t.toUser, amount: Number(amountStr) || t.amount }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['ledger', tripId] });
      setConfirming(null);
    },
  });

  const av = (id: string) => {
    const u = byId.get(id);
    return <span className="avatar sm" style={{ background: u?.avatarColor ?? '#888' }}>{u?.displayName[0] ?? '?'}</span>;
  };

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label="Settle up">
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--color-ok)' }}>🤝</span>
          <strong>Settle up</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="exp-body">
          {confirming ? (
            <div className="confirm-card">
              <h4>Record a settlement</h4>
              <div className="confirm-row">
                {av(confirming.fromUser)}
                <b>{confirming.fromUser === meId ? 'You' : name(confirming.fromUser)}</b>
                <span className="ar">paid</span>
                {av(confirming.toUser)}
                <b>{name(confirming.toUser)}</b>
                <span className="amt">{money(confirming.amount, base)}</span>
              </div>
              <label className="hint confirm-amt">
                Amount (editable — partial payments welcome)
                <span className="amount-box"><span className="cur">{currencySymbol(base)}</span><input inputMode="decimal" value={amountStr} onChange={(e) => setAmountStr(e.target.value)} /></span>
              </label>
              <div className="confirm-foot">
                <button type="button" className="btn sm" onClick={() => setConfirming(null)}>Cancel</button>
                <button type="button" className="btn accent sm" disabled={record.isPending} onClick={() => record.mutate(confirming)}>Confirm — mark settled</button>
              </div>
              <p className="hint">Writes a settlement in trip base ({base}) and drops both balances toward zero.</p>
            </div>
          ) : transfers.length === 0 ? (
            <div className="allsquare">
              <span className="em">🎉</span>
              <strong>All square</strong>
              <span className="muted">No one owes anyone — nothing left to settle.</span>
            </div>
          ) : (
            <>
              <div className="settle-head">
                <strong>{transfers.length} transfer{transfers.length === 1 ? '' : 's'} settle the whole group</strong>
                <span className="hint">amounts in trip base <b>{base}</b></span>
              </div>
              <div className="settle-list">
                {transfers.map((t, i) => {
                  const mine = t.fromUser === meId;
                  return (
                    <div key={i} className={`settle-sug${mine ? ' mine' : ''}`}>
                      {av(t.fromUser)}
                      <span className="flow">
                        <b>{mine ? 'You' : name(t.fromUser)}</b>
                        <span className="ar">→</span>
                        {av(t.toUser)}
                        <b>{name(t.toUser)}</b>
                      </span>
                      <span className="amt">{moneyWhole(t.amount, base)}</span>
                      <button type="button" className="btn accent sm" onClick={() => startConfirm(t)}>Record{mine ? ' →' : ''}</button>
                    </div>
                  );
                })}
              </div>
              <p className="hint">Only <b>your own</b> outgoing transfer is emphasised — record it yourself; the others are the group's to-do. A leader can record on anyone's behalf.</p>
            </>
          )}
        </div>
      </div>
    </SheetModal>
  );
}
