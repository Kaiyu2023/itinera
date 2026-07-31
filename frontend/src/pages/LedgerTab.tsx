import { useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useQueries } from '@tanstack/react-query';
import { Link, useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { KIND_COLOR } from './planShared';
import type { Expense, PlanDetail } from '../api/types';
import {
  AddExpenseModal,
  CATEGORY_META,
  CATEGORY_ORDER,
  Heads,
  SettleUpModal,
  fxToBase,
  money,
  moneyWhole,
  splitNote,
  splitParticipants,
  splitSummary,
} from './ledgerShared';
import type { AddExpenseSeed, StopOption } from './ledgerShared';
import type { ExpenseCategory } from '../api/types';
import { fillStyle } from '../lib/oklch';

/* ── deep links: ?ledger=add|settle, one-shot, self-stripping (Plan-tab pattern) ── */
type LedgerLink = { open: 'add' | 'settle' | null; seed: string | null; confirm: boolean };
function readLedgerDeepLink(params: URLSearchParams): LedgerLink {
  const v = params.get('ledger');
  return {
    open: v === 'add' || v === 'settle' ? v : null,
    seed: params.get('seed'),
    confirm: params.get('confirm') === 'me',
  };
}
function stripLedgerDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  ['ledger', 'seed', 'confirm'].forEach((k) => next.delete(k));
  return next;
}

/** The ledger's invalid-custom-split screenshot seed (§ mockup C). */
const CUSTOM_SEED: AddExpenseSeed = {
  amount: '27500',
  currency: 'JPY',
  category: 'lodging',
  splitMode: 'custom',
  exact: {
    'u-kaiyu': '5500',
    'u-makoto': '5500',
    'u-ryuji': '4000',
    'u-ann': '4000',
    'u-yusuke': '4000',
    'u-futaba': '2000',
  },
  note: 'Ryokan — the two private-bath rooms cost more',
};
const GYUKATSU_SEED: AddExpenseSeed = {
  amount: '14400',
  currency: 'JPY',
  category: 'food',
  linkedStopId: 's-d2-shibuya',
  note: 'Gyukatsu Motomura · Day 2 dinner — Beef-cutlet dinner in Shibuya after the Scramble, the poll winner.',
  splitMode: 'even_all',
};

/** A dropdown stop option augmented with the stream-chip label + kind colour. */
type StreamStop = StopOption & { chip: string; color: string };

type CatFilter = ExpenseCategory | 'all';
type WhoFilter = 'everyone' | 'mine' | 'insplit';
type DayFilter = 'all' | 'pretrip';

/**
 * The Ledger — the money overview (§ mockup A). Totals + soft-budget bar,
 * diverging balance bars, category / who / day filters, and the expense stream
 * with linked-stop chips + distinct settlement rows. The ＋ FAB / header
 * buttons open the add-expense and settle-up surfaces.
 */
export function LedgerTab() {
  const { tripId } = useParams();
  const api = useApi();
  const members = useMembers(tripId);
  const [params, setParams] = useSearchParams();

  const [ledger, trip, plan, me] = useQueries({
    queries: [
      { queryKey: ['ledger', tripId], queryFn: () => api.getLedger(tripId!), enabled: !!tripId },
      { queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId },
      { queryKey: ['plan', tripId], queryFn: () => api.getCurrentPlan(tripId!), enabled: !!tripId },
      { queryKey: ['me'], queryFn: () => api.getMe() },
    ],
  });

  const [cat, setCat] = useState<CatFilter>('all');
  const [who, setWho] = useState<WhoFilter>('everyone');
  const [dayF, setDayF] = useState<DayFilter>('all');

  // Surfaces + one-shot deep-link boot.
  const [surface, setSurface] = useState<
    | { kind: 'add'; seed?: AddExpenseSeed }
    | { kind: 'edit'; expense: Expense }
    | { kind: 'settle'; confirm?: boolean }
    | null
  >(null);
  const [justAdded, setJustAdded] = useState<string[]>([]);
  const booted = useRef(false);
  if (!booted.current && trip.data) {
    booted.current = true;
    const link = readLedgerDeepLink(params);
    if (link.open === 'add')
      setSurface({
        kind: 'add',
        seed: link.seed === 'custom' ? CUSTOM_SEED : link.seed === 'gyukatsu' ? GYUKATSU_SEED : undefined,
      });
    else if (link.open === 'settle') setSurface({ kind: 'settle', confirm: link.confirm });
    if (link.open) setParams(stripLedgerDeepLink(params), { replace: true });
  }

  if (ledger.isLoading || !ledger.data || !trip.data || !me.data) return <p className="muted">Loading ledger…</p>;

  const base = trip.data.baseCurrency;
  const meId = me.data.id;
  const memberList = members.data ?? [];
  const nameOf = (id: string) => members.byId.get(id)?.displayName ?? id;

  // Display currency: the currency most expenses are entered in (JPY here),
  // with the trip base as the "≈" secondary — matches the mockup's ¥ / $ read.
  const displayCurrency = dominantCurrency(ledger.data.expenses, base);
  const totalBase = ledger.data.expenses.reduce((s, e) => s + e.amount * e.fxRateToBase, 0);
  const totalDisplay = totalBase / fxToBase(displayCurrency, base);
  const memberCount = trip.data.members.length;
  // "€0.00 ≈ €0.00" was printed twice on the empty ledger: the `≈` secondary is
  // a *conversion*, and there is nothing to convert when the stream's dominant
  // currency already is the trip base.
  const showBaseAside = displayCurrency !== base;
  const isFirstRun = ledger.data.expenses.length === 0 && ledger.data.settlements.length === 0;

  // Soft budget (per-person cap × members), converted to base for the ratio.
  const sb = trip.data.softBudget;
  const capBase = sb ? sb.amount * fxToBase(sb.currency, base) * memberCount : 0;
  const budgetPct = capBase ? (totalBase / capBase) * 100 : 0;

  const stopOptions = plan.data ? buildStopOptions(plan.data) : [];
  const stopById = new Map(stopOptions.map((s) => [s.id, s]));

  const maxAbs = Math.max(1, ...ledger.data.balances.map((b) => Math.abs(b.net)));
  // Owed (negative) first? Mockup orders creditors top → debtors: sort by net desc.
  const balances = [...ledger.data.balances].sort((a, b) => b.net - a.net);
  const balanceWhole = reconcileWhole(balances.map((b) => b.net));

  // Filtered expense stream.
  const filtered = ledger.data.expenses.filter((e) => {
    if (cat !== 'all' && e.category !== cat) return false;
    if (who === 'mine' && e.paidBy !== meId) return false;
    if (who === 'insplit' && !splitParticipants(e.split).includes(meId)) return false;
    if (dayF === 'pretrip' && e.createdAt >= trip.data!.startDate) return false;
    return true;
  });
  const startDate = trip.data.startDate;
  const isPreTrip = (e: Expense) => e.createdAt < startDate && !justAdded.includes(e.id);
  const topGroup = filtered
    .filter((e) => !isPreTrip(e))
    .sort((a, b) => {
      const ai = justAdded.indexOf(a.id);
      const bi = justAdded.indexOf(b.id);
      if (ai !== -1 || bi !== -1) return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
      return b.createdAt.localeCompare(a.createdAt);
    });
  const preTrip = filtered.filter(isPreTrip);
  /**
   * Settlements obey the same filter bar as expenses.
   *
   * They used to be mapped outside every predicate, so "Paid by me" left
   * Ryuji→Ann sitting in the stream, and the empty-state guard — which only
   * counted expenses — meant "No expenses match these filters" could never
   * appear on this trip at all: the settlement row was always there to make the
   * list look non-empty. A category filter hides them outright rather than
   * inventing a category for them: a settlement is a transfer, not a spend.
   */
  const settlements = ledger.data.settlements.filter((s) => {
    if (cat !== 'all') return false;
    if (who === 'mine' && s.fromUser !== meId) return false;
    if (who === 'insplit' && s.fromUser !== meId && s.toUser !== meId) return false;
    if (dayF === 'pretrip' && s.settledAt >= startDate) return false;
    return true;
  });
  const nothingMatches = topGroup.length === 0 && preTrip.length === 0 && settlements.length === 0;
  const filtersActive = cat !== 'all' || who !== 'everyone' || dayF !== 'all';

  const renderExpense = (e: Expense) => {
    const meta = CATEGORY_META[e.category];
    const stop = e.linkedStopId ? stopById.get(e.linkedStopId) : undefined;
    const { title: noteTitle, subtitle } = splitNote(e.note);
    /**
     * A note-less expense used to render a card with no title at all — a
     * coloured icon, a category badge and an amount floating over empty space.
     *
     * Deriving the title rather than making the note mandatory: capture speed
     * is the whole point of this composer (someone standing at a till), and
     * "Food · Gyukatsu Motomura" is a perfectly good name for the row. Making
     * the note required would tax every fast entry to prevent a rendering bug.
     */
    const title = noteTitle.trim() || [meta.label, stop?.chip].filter(Boolean).join(' · ');
    const parts = splitParticipants(e.split);
    const flashed = justAdded.includes(e.id);
    const isLatestAdd = justAdded[0] === e.id;
    return (
      <div key={e.id} className={`card exp${flashed ? ' new-flash' : ''}`}>
        {/* A sibling overlay button, not a wrapping one: the row contains a
            link to the plan, and a link inside a button is invalid and
            unreachable by keyboard. The chip lifts itself above this. */}
        <button
          type="button"
          className="exp-open"
          aria-label={`Edit ${title}`}
          onClick={() => setSurface({ kind: 'edit', expense: e })}
        />
        <span className="cat-ic" style={{ background: meta.color }}>
          {meta.emoji}
        </span>
        <div className="exp-main">
          <div className="exp-title">
            <strong>{title}</strong>
            <span className="badge">{e.category}</span>
            {e.currency !== base && e.currency !== displayCurrency && <span className="badge money">{e.currency}</span>}
            {isLatestAdd && (
              <span className="badge ok" style={{ fontSize: '0.6rem' }}>
                just added
              </span>
            )}
          </div>
          {subtitle && <div className="exp-note">{subtitle}</div>}
          <div className="exp-meta">
            {/* The separator lives at the *end* of the preceding item. Baked
                onto the front of the next one it started wrapped lines with a
                naked "· split 6 ways". */}
            <span>
              paid by <b>{nameOf(e.paidBy)}</b> ·
            </span>
            <span>{splitSummary(e.split, e.amount, e.currency)}</span>
            <Heads ids={parts} membersById={members.byId} />
            {e.currency === base && showBaseAside && <span>entered in {base}</span>}
            {stop && (
              <Link to="../plan" className="stop-chip">
                <span className="kd" style={{ background: stop.color } as CSSProperties} />
                {stop.chip} ↗
              </Link>
            )}
          </div>
        </div>
        <div className="exp-amt">
          <div className="big">{money(e.amount, e.currency)}</div>
          {/* This slot is the base-currency conversion. A base-currency row has
              no conversion, and the prose "entered in USD" that sat here broke
              the numeric column — an em dash holds the row's rhythm and the
              fact moved up into the meta line where prose belongs. */}
          <div className="base">{e.currency === base ? '—' : '≈ ' + money(e.amount * e.fxRateToBase, base)}</div>
          <div className="pp">{formatShortDate(e.createdAt)}</div>
        </div>
      </div>
    );
  };

  return (
    <div className="m4-tab">
      <div className="m4-tab-head">
        <h1>Ledger</h1>
        <span className="spacer" />
        <button type="button" className="btn" onClick={() => setSurface({ kind: 'settle' })}>
          Settle up
        </button>
        {/* Hidden below 720px: the FAB is the mobile add affordance, and the
            two were ~600px apart on a phone — the same primary action twice,
            never both in view, so neither one taught you where "add" lives. */}
        <button type="button" className="btn accent m4-head-add" onClick={() => setSurface({ kind: 'add' })}>
          ＋ Add expense
        </button>
      </div>

      {isFirstRun ? (
        <FirstRunLedger base={base} onAdd={() => setSurface({ kind: 'add' })} />
      ) : (
        <>
          {/* Totals + soft-budget bar */}
          <div className="card" style={{ display: 'grid', gap: 6 }}>
            <div className="stat-row">
              <div className="stat">
                <div className="k">Trip total so far</div>
                <div className="v">
                  {money(totalDisplay, displayCurrency)}{' '}
                  {showBaseAside && <small>&#8776;&nbsp;{money(totalBase, base)}</small>}
                </div>
              </div>
              <div className="stat">
                <div className="k">Per person</div>
                <div className="v">
                  {money(totalDisplay / memberCount, displayCurrency)}{' '}
                  {showBaseAside && <small>&#8776;&nbsp;{money(totalBase / memberCount, base)}</small>}
                </div>
              </div>
              <div className="stat">
                <div className="k">Expenses logged</div>
                <div className="v">
                  {ledger.data.expenses.length}{' '}
                  <small>
                    + {ledger.data.settlements.length} settlement{ledger.data.settlements.length === 1 ? '' : 's'}
                  </small>
                </div>
              </div>
            </div>
            {sb && (
              <div className="budget">
                {/* The cap tick sat at `left: 100%` inside an `overflow: hidden`
                track — a rendered element that could never be seen, the one
                mark on the bar that says what "100%" is against. It now sits
                just inside the end of the track, with the cap figure written
                out underneath so the bar has a scale and not just a fraction. */}
                <div className="budget-track">
                  <div
                    className={`budget-fill${budgetPct > 100 ? ' over' : ''}`}
                    style={{ width: `${Math.min(100, budgetPct)}%` }}
                  />
                  <div className="budget-cap" />
                </div>
                <div className="budget-legend">
                  <span>
                    {Math.round(budgetPct)}% of the group's <b>{money(sb.amount, sb.currency)} / person</b> soft cap
                  </span>
                  <span className="budget-capline">
                    cap <b>{moneyWhole(capBase, base)}</b> for {memberCount} · never blocks a spend
                  </span>
                </div>
              </div>
            )}
          </div>

          {/* Diverging balances */}
          <div className="sub-anno">Balances — owes ↤ centre ↦ is owed</div>
          <div className="card">
            <div className="axis-key">
              <span>owes the group</span>
              <span>is owed</span>
            </div>
            <div className="bal-list">
              {balances.map((b, i) => {
                // Displayed figure, not the raw net: the column has to add to zero
                // (see reconcileWhole), and the sign/colour must agree with what is
                // actually printed. A −$0.40 net that prints as $0 is settled.
                const shown = balanceWhole[i];
                const tone = shown === 0 ? 'zero' : shown > 0 ? 'pos' : 'neg';
                const width = (Math.abs(b.net) / maxAbs) * 50;
                return (
                  <div key={b.userId} className="bal-row">
                    <span className="bal-who">
                      <span className="avatar sm" style={fillStyle(members.byId.get(b.userId)?.avatarColor ?? '#888')}>
                        {nameOf(b.userId)[0]}
                      </span>
                      <span className="nm">
                        {nameOf(b.userId)}
                        {b.userId === meId ? ' (you)' : ''}
                      </span>
                    </span>
                    <span className="bal-track">
                      {tone !== 'zero' && <span className={`bal-fill ${tone}`} style={{ width: `${width}%` }} />}
                    </span>
                    {/* "+€0" in credit green told a settled-up group that all six of
                    them were owed money. Zero has no direction and no sign. */}
                    <span className={`bal-amt ${tone}`}>
                      {tone === 'zero' ? (
                        'settled'
                      ) : (
                        <>
                          {shown > 0 ? '+' : '−'}
                          {moneyWhole(Math.abs(shown), base)}
                        </>
                      )}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>

          {/* Filters */}
          <div className="card" style={{ marginBottom: 2 }}>
            <div className="filter-bar">
              <span className="fl">Category</span>
              <button type="button" className={`fchip${cat === 'all' ? ' on' : ''}`} onClick={() => setCat('all')}>
                All
              </button>
              {CATEGORY_ORDER.map((c) => (
                <button key={c} type="button" className={`fchip${cat === c ? ' on' : ''}`} onClick={() => setCat(c)}>
                  <span className="kd" style={{ background: CATEGORY_META[c].color }} />
                  {CATEGORY_META[c].label}
                </button>
              ))}
            </div>
            <div className="filter-bar">
              <span className="fl">Who</span>
              <button
                type="button"
                className={`fchip${who === 'everyone' ? ' on' : ''}`}
                onClick={() => setWho('everyone')}
              >
                Everyone
              </button>
              <button type="button" className={`fchip${who === 'mine' ? ' on' : ''}`} onClick={() => setWho('mine')}>
                Paid by me
              </button>
              <button
                type="button"
                className={`fchip${who === 'insplit' ? ' on' : ''}`}
                onClick={() => setWho('insplit')}
              >
                In my splits
              </button>
              <span className="fl gap">Day</span>
              <button type="button" className={`fchip${dayF === 'all' ? ' on' : ''}`} onClick={() => setDayF('all')}>
                All
              </button>
              <button
                type="button"
                className={`fchip${dayF === 'pretrip' ? ' on' : ''}`}
                onClick={() => setDayF('pretrip')}
              >
                Pre-trip
              </button>
            </div>
          </div>

          {/* Expense stream */}
          <div className="exp-list">
            {nothingMatches &&
              (filtersActive ? (
                <p className="exp-none">
                  Nothing matches these filters.{' '}
                  <button
                    type="button"
                    className="linkish"
                    onClick={() => {
                      setCat('all');
                      setWho('everyone');
                      setDayF('all');
                    }}
                  >
                    Clear all three
                  </button>
                </p>
              ) : (
                // Reachable only when every expense was deleted but a settlement
                // survives; the no-filter, no-anything case is the first-run block.
                <p className="exp-none">No expenses logged yet.</p>
              ))}
            {topGroup.map(renderExpense)}
            {preTrip.length > 0 && <div className="day-sep">Pre-trip bookings</div>}
            {preTrip.map(renderExpense)}
            {settlements.map((s) => (
              <div key={s.id} className="settle-entry">
                <span className="hand">🤝</span>
                <span className="txt">
                  <b>{nameOf(s.fromUser)}</b> paid <b>{nameOf(s.toUser)}</b>{' '}
                  <span className="amt">{money(s.amount, base)}</span>
                </span>
                <span className="tag">settled {formatShortDate(s.settledAt)}</span>
              </div>
            ))}
          </div>
        </>
      )}

      <button type="button" className="m4-fab" onClick={() => setSurface({ kind: 'add' })} aria-label="Add expense">
        ＋
      </button>

      {surface?.kind === 'add' && (
        <AddExpenseModal
          members={memberList}
          meId={meId}
          tripId={tripId!}
          base={base}
          stops={stopOptions}
          seed={surface.seed}
          onClose={() => setSurface(null)}
          onAdded={(id) => setJustAdded((prev) => [id, ...prev])}
        />
      )}
      {surface?.kind === 'edit' && (
        <AddExpenseModal
          // Keyed on the row so switching rows remounts with fresh field state
          // rather than carrying the previous expense's amount across.
          key={surface.expense.id}
          members={memberList}
          meId={meId}
          tripId={tripId!}
          base={base}
          stops={stopOptions}
          expense={surface.expense}
          onClose={() => setSurface(null)}
          onAdded={() => {}}
        />
      )}
      {surface?.kind === 'settle' && (
        <SettleUpModal
          transfers={ledger.data.suggestedTransfers}
          members={memberList}
          meId={meId}
          tripId={tripId!}
          base={base}
          initialConfirm={surface.confirm}
          onClose={() => setSurface(null)}
        />
      )}
    </div>
  );
}

/* ── helpers ── */

/**
 * The ledger before anything has been spent.
 *
 * What used to render here: three zero-width balance bars under a "owes ↤
 * centre ↦ is owed" axis nobody could read, ten filter chips filtering nothing,
 * "€0.00 ≈ €0.00" twice, and "No expenses match these filters." with no filter
 * set. Every one of those controls describes data that doesn't exist yet, so
 * the whole body is replaced by the one thing there is to do.
 */
function FirstRunLedger({ base, onAdd }: { base: string; onAdd: () => void }) {
  return (
    <div className="card ledger-firstrun">
      <span className="em" aria-hidden>
        🧾
      </span>
      <strong>No expenses yet</strong>
      <p className="muted">
        Log what anyone pays for and Itinera keeps the running total, splits it however you like, and works out the
        fewest transfers that square everyone up. Amounts convert to <b>{base}</b> at the rate on the day you enter
        them.
      </p>
      <button type="button" className="btn accent" onClick={onAdd}>
        ＋ Add the first expense
      </button>
      <span className="hint">
        Balances, the soft-budget bar and settle-up appear once there's something to balance.
      </span>
    </div>
  );
}

/**
 * Whole-unit amounts that still sum to the whole-unit total.
 *
 * The balance column printed +$830 +$542 −$190 −$301 −$427 −$455 = −$1: six
 * independent `Math.round`s, each off by up to 50c, with nothing making the
 * errors cancel — and a balance column that doesn't reach zero looks like the
 * ledger has lost a dollar. Largest-remainder apportionment instead: floor
 * every row, then give the leftover whole units to the rows with the largest
 * discarded fraction. Every printed figure is within 1 of its true value and
 * the column reconciles exactly.
 */
function reconcileWhole(values: number[]): number[] {
  const floors = values.map((v) => Math.floor(v));
  const out = [...floors];
  const target = Math.round(values.reduce((s, v) => s + v, 0));
  const leftover = target - floors.reduce((s, v) => s + v, 0);
  const byFraction = values.map((v, i) => ({ i, frac: v - Math.floor(v) })).sort((a, b) => b.frac - a.frac);
  for (let k = 0; k < leftover && byFraction.length > 0; k++) out[byFraction[k % byFraction.length].i] += 1;
  return out;
}

function dominantCurrency(expenses: Expense[], base: string): string {
  const counts = new Map<string, number>();
  for (const e of expenses) counts.set(e.currency, (counts.get(e.currency) ?? 0) + 1);
  let best = base;
  let n = 0;
  for (const [c, k] of counts)
    if (k > n) {
      best = c;
      n = k;
    }
  return best;
}

function formatShortDate(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

/** Stop options for the linked-stop dropdown + the stream chips. */
function buildStopOptions(detail: PlanDetail): StreamStop[] {
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const dayIndex = new Map(days.map((d, i) => [d.id, i + 1]));
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? 'stop';
  return [...detail.stops]
    .sort((a, b) => {
      const da = dayIndex.get(a.dayId) ?? 0;
      const db = dayIndex.get(b.dayId) ?? 0;
      return da - db || a.seq - b.seq;
    })
    .map((s) => {
      const n = dayIndex.get(s.dayId) ?? 0;
      const day = days.find((d) => d.id === s.dayId);
      const name = placeName(s.placeId);
      const date = day
        ? new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
        : '';
      return {
        id: s.id,
        stopKind: s.stopKind,
        note: s.notes,
        label: `Day ${n} · ${name} (${s.stopKind}) — ${date}`,
        chip: `Day ${n} · ${shortName(name)}`,
        color: KIND_COLOR[s.stopKind],
      };
    });
}

/** A compact chip label from a place name — drop parentheticals + a leading
    generic word ("Hotel"), then keep the first distinctive word. */
function shortName(name: string): string {
  const words = name.split(' (')[0].split(' ');
  if (words.length > 1 && /^(hotel|the|café|cafe|restaurant)$/i.test(words[0])) words.shift();
  return words[0];
}
