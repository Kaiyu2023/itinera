import { useState } from 'react';
import type { CSSProperties } from 'react';
import { useQueries } from '@tanstack/react-query';
import { Link, useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import { useMembers } from '../components/hooks';
import { KIND_COLOR } from './planShared';
import type { Expense, PlanDetail } from '../api/types';
import { AddExpenseModal, Heads, SettleUpModal } from './ledgerShared';
import {
  CATEGORY_META,
  CATEGORY_ORDER,
  fxToBase,
  money,
  moneyWhole,
  splitNote,
  splitParticipants,
  splitSummary,
} from './ledgerDomain';
import type { AddExpenseSeed, StopOption } from './ledgerDomain';
import type { ExpenseCategory } from '../api/types';
import { fillStyle } from '../lib/oklch';
import { useI18n } from '../i18n';
import { useOneShotDeepLink } from '../lib/useOneShotDeepLink';

/* ── deep links: ?ledger=add|settle, one-shot, self-stripping (Plan-tab pattern) ── */
type LedgerLink = { open: 'add' | 'settle'; seed: string | null; confirm: boolean };
function readLedgerDeepLink(params: URLSearchParams): LedgerLink | null {
  const v = params.get('ledger');
  if (v !== 'add' && v !== 'settle') return null;
  return {
    open: v,
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
  const { locale, t: ui, formatDate } = useI18n();
  const formatNumber = (value: number, options?: Intl.NumberFormatOptions) =>
    new Intl.NumberFormat(locale, options).format(value);
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
  useOneShotDeepLink({
    ready: !!trip.data,
    searchParams: params,
    setSearchParams: setParams,
    read: readLedgerDeepLink,
    strip: stripLedgerDeepLink,
    onMatch: (link) => {
      if (link.open === 'add') {
        setSurface({
          kind: 'add',
          seed: link.seed === 'custom' ? CUSTOM_SEED : link.seed === 'gyukatsu' ? GYUKATSU_SEED : undefined,
        });
      } else if (link.open === 'settle') {
        setSurface({ kind: 'settle', confirm: link.confirm });
      }
    },
  });

  if (ledger.isLoading || !ledger.data || !trip.data || !me.data)
    return <p className="muted">{ui('ledger.loading')}</p>;

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

  const stopOptions = plan.data ? buildStopOptions(plan.data, ui, formatDate) : [];
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
    const title = noteTitle.trim() || [ui(meta.labelKey), stop?.chip].filter(Boolean).join(' · ');
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
          aria-label={`${ui('ledger.editExpenseAria')} ${title}`}
          onClick={() => setSurface({ kind: 'edit', expense: e })}
        />
        <span className="cat-ic" style={{ background: meta.color }}>
          {meta.emoji}
        </span>
        <div className="exp-main">
          <div className="exp-title">
            <strong>{title}</strong>
            <span className="badge">{ui(meta.labelKey)}</span>
            {e.currency !== base && e.currency !== displayCurrency && <span className="badge money">{e.currency}</span>}
            {isLatestAdd && (
              <span className="badge ok" style={{ fontSize: 'var(--type-micro)' }}>
                {ui('ledger.justAdded')}
              </span>
            )}
          </div>
          {subtitle && <div className="exp-note">{subtitle}</div>}
          <div className="exp-meta">
            {/* The separator lives at the *end* of the preceding item. Baked
                onto the front of the next one it started wrapped lines with a
                naked "· split 6 ways". */}
            <span>
              {ui('ledger.paidBy')} <b>{nameOf(e.paidBy)}</b> ·
            </span>
            <span>{splitSummary(e.split, e.amount, e.currency, locale, ui)}</span>
            <Heads ids={parts} membersById={members.byId} />
            {e.currency === base && showBaseAside && <span>{ui('ledger.enteredIn', { currency: base })}</span>}
            {stop && (
              <Link to="../plan" className="stop-chip">
                <span className="kd" style={{ background: stop.color } as CSSProperties} />
                {stop.chip} ↗
              </Link>
            )}
          </div>
        </div>
        <div className="exp-amt">
          <div className="big">{money(e.amount, e.currency, locale)}</div>
          {/* This slot is the base-currency conversion. A base-currency row has
              no conversion, and the prose "entered in USD" that sat here broke
              the numeric column — an em dash holds the row's rhythm and the
              fact moved up into the meta line where prose belongs. */}
          <div className="base">
            {e.currency === base ? '—' : '≈ ' + money(e.amount * e.fxRateToBase, base, locale)}
          </div>
          <div className="pp">{formatDate(e.createdAt, { month: 'short', day: 'numeric' })}</div>
        </div>
      </div>
    );
  };

  return (
    <div className="m4-tab">
      <div className="m4-tab-head">
        <h2>{ui('ledger.title')}</h2>
        <span className="spacer" />
        <button type="button" className="btn" onClick={() => setSurface({ kind: 'settle' })}>
          {ui('ledger.settleUp')}
        </button>
        {/* Hidden below 720px: the FAB is the mobile add affordance, and the
            two were ~600px apart on a phone — the same primary action twice,
            never both in view, so neither one taught you where "add" lives. */}
        <button type="button" className="btn accent m4-head-add" onClick={() => setSurface({ kind: 'add' })}>
          ＋ {ui('ledger.addExpense')}
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
                <div className="k">{ui('ledger.tripTotal')}</div>
                <div className="v">
                  {money(totalDisplay, displayCurrency, locale)}{' '}
                  {showBaseAside && <small>&#8776;&nbsp;{money(totalBase, base, locale)}</small>}
                </div>
              </div>
              <div className="stat">
                <div className="k">{ui('ledger.perPerson')}</div>
                <div className="v">
                  {money(totalDisplay / memberCount, displayCurrency, locale)}{' '}
                  {showBaseAside && <small>&#8776;&nbsp;{money(totalBase / memberCount, base, locale)}</small>}
                </div>
              </div>
              <div className="stat">
                <div className="k">{ui('ledger.expensesLogged')}</div>
                <div className="v">
                  {formatNumber(ledger.data.expenses.length)}{' '}
                  <small>
                    + {formatNumber(ledger.data.settlements.length)}{' '}
                    {ui(ledger.data.settlements.length === 1 ? 'ledger.settlement.one' : 'ledger.settlement.many')}
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
                    {ui('ledger.budget.progressPrefix', {
                      percent: formatNumber(budgetPct, { maximumFractionDigits: 0 }),
                    })}{' '}
                    <b>{ui('ledger.budget.softCap', { amount: money(sb.amount, sb.currency, locale) })}</b>
                  </span>
                  <span className="budget-capline">
                    {ui('ledger.budget.capSummary', {
                      amount: moneyWhole(capBase, base, locale),
                      count: formatNumber(memberCount),
                    })}
                  </span>
                </div>
              </div>
            )}
          </div>

          {/* Diverging balances */}
          <div className="sub-anno">{ui('ledger.balancesCaption')}</div>
          <div className="card">
            <div className="axis-key">
              <span>{ui('ledger.owesGroup')}</span>
              <span>{ui('ledger.isOwed')}</span>
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
                        {b.userId === meId ? ` (${ui('ledger.you')})` : ''}
                      </span>
                    </span>
                    <span className="bal-track">
                      {tone !== 'zero' && <span className={`bal-fill ${tone}`} style={{ width: `${width}%` }} />}
                    </span>
                    {/* "+€0" in credit green told a settled-up group that all six of
                    them were owed money. Zero has no direction and no sign. */}
                    <span className={`bal-amt ${tone}`}>
                      {tone === 'zero' ? (
                        ui('ledger.settled')
                      ) : (
                        <>
                          {shown > 0 ? '+' : '−'}
                          {moneyWhole(Math.abs(shown), base, locale)}
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
              <span className="fl">{ui('ledger.filter.category')}</span>
              <button
                type="button"
                className={`fchip${cat === 'all' ? ' on' : ''}`}
                aria-pressed={cat === 'all'}
                onClick={() => setCat('all')}
              >
                {ui('ledger.filter.all')}
              </button>
              {CATEGORY_ORDER.map((c) => (
                <button
                  key={c}
                  type="button"
                  className={`fchip${cat === c ? ' on' : ''}`}
                  aria-pressed={cat === c}
                  onClick={() => setCat(c)}
                >
                  <span className="kd" style={{ background: CATEGORY_META[c].color }} />
                  {ui(CATEGORY_META[c].labelKey)}
                </button>
              ))}
            </div>
            <div className="filter-bar">
              <span className="fl">{ui('ledger.filter.who')}</span>
              <button
                type="button"
                className={`fchip${who === 'everyone' ? ' on' : ''}`}
                aria-pressed={who === 'everyone'}
                onClick={() => setWho('everyone')}
              >
                {ui('ledger.filter.everyone')}
              </button>
              <button
                type="button"
                className={`fchip${who === 'mine' ? ' on' : ''}`}
                aria-pressed={who === 'mine'}
                onClick={() => setWho('mine')}
              >
                {ui('ledger.filter.paidByMe')}
              </button>
              <button
                type="button"
                className={`fchip${who === 'insplit' ? ' on' : ''}`}
                aria-pressed={who === 'insplit'}
                onClick={() => setWho('insplit')}
              >
                {ui('ledger.filter.inMySplits')}
              </button>
              <span className="fl gap">{ui('ledger.filter.day')}</span>
              <button
                type="button"
                className={`fchip${dayF === 'all' ? ' on' : ''}`}
                aria-pressed={dayF === 'all'}
                onClick={() => setDayF('all')}
              >
                {ui('ledger.filter.all')}
              </button>
              <button
                type="button"
                className={`fchip${dayF === 'pretrip' ? ' on' : ''}`}
                aria-pressed={dayF === 'pretrip'}
                onClick={() => setDayF('pretrip')}
              >
                {ui('ledger.filter.preTrip')}
              </button>
            </div>
          </div>

          {/* Expense stream */}
          <div className="exp-list">
            {nothingMatches &&
              (filtersActive ? (
                <p className="exp-none">
                  {ui('ledger.filter.none')}{' '}
                  <button
                    type="button"
                    className="linkish"
                    onClick={() => {
                      setCat('all');
                      setWho('everyone');
                      setDayF('all');
                    }}
                  >
                    {ui('ledger.filter.clear')}
                  </button>
                </p>
              ) : (
                // Reachable only when every expense was deleted but a settlement
                // survives; the no-filter, no-anything case is the first-run block.
                <p className="exp-none">{ui('ledger.empty.logged')}</p>
              ))}
            {topGroup.map(renderExpense)}
            {preTrip.length > 0 && <div className="day-sep">{ui('ledger.preTripBookings')}</div>}
            {preTrip.map(renderExpense)}
            {settlements.map((s) => (
              <div key={s.id} className="settle-entry">
                <span className="hand">🤝</span>
                <span className="txt">
                  <b>{nameOf(s.fromUser)}</b> {ui('ledger.settlement.paid')} <b>{nameOf(s.toUser)}</b>{' '}
                  <span className="amt">{money(s.amount, base, locale)}</span>
                </span>
                <span className="tag">
                  {ui('ledger.settlement.date', {
                    date: formatDate(s.settledAt, { month: 'short', day: 'numeric' }),
                  })}
                </span>
              </div>
            ))}
          </div>
        </>
      )}

      <button
        type="button"
        className="m4-fab"
        onClick={() => setSurface({ kind: 'add' })}
        aria-label={ui('ledger.addExpense')}
      >
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
  const { t: ui } = useI18n();
  return (
    <div className="card ledger-firstrun">
      <span className="em" aria-hidden>
        🧾
      </span>
      <strong>{ui('ledger.empty.title')}</strong>
      <p className="muted">
        {ui('ledger.empty.description')} <b>{base}</b> {ui('ledger.empty.rateSuffix')}
      </p>
      <button type="button" className="btn accent" onClick={onAdd}>
        ＋ {ui('ledger.empty.addFirst')}
      </button>
      <span className="hint">{ui('ledger.empty.hint')}</span>
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

/** Stop options for the linked-stop dropdown + the stream chips. */
function buildStopOptions(
  detail: PlanDetail,
  ui: ReturnType<typeof useI18n>['t'],
  formatDate: ReturnType<typeof useI18n>['formatDate'],
): StreamStop[] {
  const days = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const dayIndex = new Map(days.map((d, i) => [d.id, i + 1]));
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? ui('ledger.stop.fallback');
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
      const date = day ? formatDate(day.date, { month: 'short', day: 'numeric' }) : '';
      const dayLabel = ui('ledger.dayNumber', { day: n });
      const kindLabel = localizedStopKind(s.stopKind, ui);
      return {
        id: s.id,
        stopKind: s.stopKind,
        note: s.notes,
        label: `${dayLabel} · ${name} (${kindLabel}) — ${date}`,
        chip: `${dayLabel} · ${shortName(name)}`,
        color: KIND_COLOR[s.stopKind],
      };
    });
}

function localizedStopKind(kind: string, ui: ReturnType<typeof useI18n>['t']): string {
  if (kind === 'lodging') return ui('ledger.stop.kind.lodging');
  if (kind === 'meal') return ui('ledger.stop.kind.meal');
  if (kind === 'transit') return ui('ledger.stop.kind.transit');
  if (kind === 'activity') return ui('ledger.stop.kind.activity');
  if (kind === 'visit') return ui('ledger.stop.kind.visit');
  return kind;
}

/** A compact chip label from a place name — drop parentheticals + a leading
    generic word ("Hotel"), then keep the first distinctive word. */
function shortName(name: string): string {
  const words = name.split(' (')[0].split(' ');
  if (words.length > 1 && /^(hotel|the|café|cafe|restaurant)$/i.test(words[0])) words.shift();
  return words[0];
}
