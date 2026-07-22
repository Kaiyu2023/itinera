import { useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useQueries } from '@tanstack/react-query';
import { Link, useParams, useSearchParams } from 'react-router-dom';
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
  exact: { 'u-kaiyu': '5500', 'u-makoto': '5500', 'u-ryuji': '4000', 'u-ann': '4000', 'u-yusuke': '4000', 'u-futaba': '2000' },
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
  const [surface, setSurface] = useState<{ kind: 'add'; seed?: AddExpenseSeed } | { kind: 'settle'; confirm?: boolean } | null>(null);
  const [justAdded, setJustAdded] = useState<string[]>([]);
  const booted = useRef(false);
  if (!booted.current && trip.data) {
    booted.current = true;
    const link = readLedgerDeepLink(params);
    if (link.open === 'add') setSurface({ kind: 'add', seed: link.seed === 'custom' ? CUSTOM_SEED : link.seed === 'gyukatsu' ? GYUKATSU_SEED : undefined });
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

  // Soft budget (per-person cap × members), converted to base for the ratio.
  const sb = trip.data.softBudget;
  const capBase = sb ? sb.amount * fxToBase(sb.currency, base) * memberCount : 0;
  const budgetPct = capBase ? (totalBase / capBase) * 100 : 0;

  const stopOptions = plan.data ? buildStopOptions(plan.data) : [];
  const stopById = new Map(stopOptions.map((s) => [s.id, s]));

  const maxAbs = Math.max(1, ...ledger.data.balances.map((b) => Math.abs(b.net)));
  // Owed (negative) first? Mockup orders creditors top → debtors: sort by net desc.
  const balances = [...ledger.data.balances].sort((a, b) => b.net - a.net);

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
  const topGroup = filtered.filter((e) => !isPreTrip(e)).sort((a, b) => {
    const ai = justAdded.indexOf(a.id);
    const bi = justAdded.indexOf(b.id);
    if (ai !== -1 || bi !== -1) return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    return b.createdAt.localeCompare(a.createdAt);
  });
  const preTrip = filtered.filter(isPreTrip);
  const settlements = ledger.data.settlements;

  const renderExpense = (e: Expense) => {
    const meta = CATEGORY_META[e.category];
    const { title, subtitle } = splitNote(e.note);
    const parts = splitParticipants(e.split);
    const stop = e.linkedStopId ? stopById.get(e.linkedStopId) : undefined;
    const flashed = justAdded.includes(e.id);
    const isLatestAdd = justAdded[0] === e.id;
    return (
      <div key={e.id} className={`card exp${flashed ? ' new-flash' : ''}`}>
        <span className="cat-ic" style={{ background: meta.color }}>{meta.emoji}</span>
        <div className="exp-main">
          <div className="exp-title">
            <strong>{title}</strong>
            <span className="badge">{e.category}</span>
            {e.currency !== base && e.currency !== displayCurrency && <span className="badge money">{e.currency}</span>}
            {isLatestAdd && <span className="badge ok" style={{ fontSize: '0.6rem' }}>just added</span>}
          </div>
          {subtitle && <div className="exp-note">{subtitle}</div>}
          <div className="exp-meta">
            <span>paid by <b>{nameOf(e.paidBy)}</b></span>
            <span>· {splitSummary(e.split, e.amount, e.currency)}</span>
            <Heads ids={parts} membersById={members.byId} />
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
          <div className="base">{e.currency === base ? 'entered in ' + base : '≈ ' + money(e.amount * e.fxRateToBase, base)}</div>
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
        <button type="button" className="btn" onClick={() => setSurface({ kind: 'settle' })}>Settle up</button>
        <button type="button" className="btn accent" onClick={() => setSurface({ kind: 'add' })}>＋ Add expense</button>
      </div>

      {/* Totals + soft-budget bar */}
      <div className="card" style={{ display: 'grid', gap: 6 }}>
        <div className="stat-row">
          <div className="stat"><div className="k">Trip total so far</div><div className="v">{money(totalDisplay, displayCurrency)} <small>≈ {money(totalBase, base)}</small></div></div>
          <div className="stat"><div className="k">Per person</div><div className="v">{money(totalDisplay / memberCount, displayCurrency)} <small>≈ {money(totalBase / memberCount, base)}</small></div></div>
          <div className="stat"><div className="k">Expenses logged</div><div className="v">{ledger.data.expenses.length} <small>+ {settlements.length} settlement{settlements.length === 1 ? '' : 's'}</small></div></div>
        </div>
        {sb && (
          <div className="budget">
            <div className="budget-track">
              <div className={`budget-fill${budgetPct > 100 ? ' over' : ''}`} style={{ width: `${Math.min(100, budgetPct)}%` }} />
              <div className="budget-cap" style={{ left: '100%' }} />
            </div>
            <div className="budget-legend">
              <span>{Math.round(budgetPct)}% of the group's <b>{money(sb.amount, sb.currency)} / person</b> soft cap</span>
              <span>never blocks a spend — just a heads-up</span>
            </div>
          </div>
        )}
      </div>

      {/* Diverging balances */}
      <div className="sub-anno">Balances — owes ↤ centre ↦ is owed</div>
      <div className="card">
        <div className="axis-key"><span>owes the group</span><span>is owed</span></div>
        <div className="bal-list">
          {balances.map((b) => {
            const pos = b.net >= 0;
            const width = (Math.abs(b.net) / maxAbs) * 50;
            return (
              <div key={b.userId} className="bal-row">
                <span className="bal-who">
                  <span className="avatar sm" style={{ background: members.byId.get(b.userId)?.avatarColor ?? '#888' }}>{nameOf(b.userId)[0]}</span>
                  <span className="nm">{nameOf(b.userId)}{b.userId === meId ? ' (you)' : ''}</span>
                </span>
                <span className="bal-track"><span className={`bal-fill ${pos ? 'pos' : 'neg'}`} style={{ width: `${width}%` }} /></span>
                <span className={`bal-amt ${pos ? 'pos' : 'neg'}`}>{pos ? '+' : '−'}{moneyWhole(Math.abs(b.net), base)}</span>
              </div>
            );
          })}
        </div>
      </div>

      {/* Filters */}
      <div className="card" style={{ marginBottom: 2 }}>
        <div className="filter-bar">
          <span className="fl">Category</span>
          <button type="button" className={`fchip${cat === 'all' ? ' on' : ''}`} onClick={() => setCat('all')}>All</button>
          {CATEGORY_ORDER.map((c) => (
            <button key={c} type="button" className={`fchip${cat === c ? ' on' : ''}`} onClick={() => setCat(c)}>
              <span className="kd" style={{ background: CATEGORY_META[c].color }} />{CATEGORY_META[c].label}
            </button>
          ))}
        </div>
        <div className="filter-bar">
          <span className="fl">Who</span>
          <button type="button" className={`fchip${who === 'everyone' ? ' on' : ''}`} onClick={() => setWho('everyone')}>Everyone</button>
          <button type="button" className={`fchip${who === 'mine' ? ' on' : ''}`} onClick={() => setWho('mine')}>Paid by me</button>
          <button type="button" className={`fchip${who === 'insplit' ? ' on' : ''}`} onClick={() => setWho('insplit')}>In my splits</button>
          <span className="fl gap">Day</span>
          <button type="button" className={`fchip${dayF === 'all' ? ' on' : ''}`} onClick={() => setDayF('all')}>All</button>
          <button type="button" className={`fchip${dayF === 'pretrip' ? ' on' : ''}`} onClick={() => setDayF('pretrip')}>Pre-trip</button>
        </div>
      </div>

      {/* Expense stream */}
      <div className="exp-list">
        {topGroup.length === 0 && preTrip.length === 0 && <p className="muted">No expenses match these filters.</p>}
        {topGroup.map(renderExpense)}
        {preTrip.length > 0 && <div className="day-sep">Pre-trip bookings</div>}
        {preTrip.map(renderExpense)}
        {settlements.map((s) => (
          <div key={s.id} className="settle-entry">
            <span className="hand">🤝</span>
            <span className="txt"><b>{nameOf(s.fromUser)}</b> paid <b>{nameOf(s.toUser)}</b> <span className="amt">{money(s.amount, base)}</span></span>
            <span className="tag">settled {formatShortDate(s.settledAt)}</span>
          </div>
        ))}
      </div>

      <button type="button" className="m4-fab" onClick={() => setSurface({ kind: 'add' })} aria-label="Add expense">＋</button>

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

function dominantCurrency(expenses: Expense[], base: string): string {
  const counts = new Map<string, number>();
  for (const e of expenses) counts.set(e.currency, (counts.get(e.currency) ?? 0) + 1);
  let best = base;
  let n = 0;
  for (const [c, k] of counts) if (k > n) { best = c; n = k; }
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
      const date = day ? new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';
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
