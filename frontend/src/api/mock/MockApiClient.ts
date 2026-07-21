import type {
  AddCandidateInput,
  AddExpenseInput,
  AddSettlementInput,
  ApiClient,
  CreateNoticeInput,
  CreatePollInput,
  CreateProposalInput,
  CreateTokenInput,
  CreateTripInput,
  DayPatch,
  NoticePatch,
  StopPatch,
} from '../client';
import type {
  ApiToken,
  Candidate,
  CandidateWithPlace,
  Comment,
  CreatedToken,
  Day,
  Edit,
  Expense,
  Invite,
  LedgerView,
  Notice,
  Place,
  Plan,
  PlanDetail,
  Poll,
  Proposal,
  ReviewItem,
  Settlement,
  Stop,
  Thread,
  Trip,
  TripSummary,
  User,
} from '../types';
import * as fixtures from './fixtures';

/**
 * In-memory ApiClient used throughout Phase A. Mutations actually mutate the
 * store so the UI is fully interactive; state resets on page reload.
 * Simulated latency keeps loading states honest.
 */
export class MockApiClient implements ApiClient {
  private users: User[] = clone(fixtures.users);
  private trips: Trip[] = [clone(fixtures.trip), ...clone(fixtures.dreamTrips)];
  private invites: Invite[] = [];
  private places: Place[] = clone(fixtures.places);
  private candidates: Candidate[] = clone(fixtures.candidates);
  private plans: Plan[] = clone(fixtures.planVersions);
  private days: Day[] = clone(fixtures.days);
  private stops: Stop[] = clone(fixtures.stops);
  private legs = clone(fixtures.legs);
  private dayFeasibility = clone(fixtures.dayFeasibility);
  private proposals: Proposal[] = clone(fixtures.proposals);
  private polls: Poll[] = clone(fixtures.polls);
  private edits: Edit[] = clone(fixtures.edits);
  private reviewItems: ReviewItem[] = clone(fixtures.reviewItems);
  private threads: Thread[] = clone(fixtures.threads);
  private comments: Comment[] = clone(fixtures.comments);
  private expenses: Expense[] = clone(fixtures.expenses);
  private settlements: Settlement[] = clone(fixtures.settlements);
  private notices: Notice[] = clone(fixtures.notices);
  private tokens: ApiToken[] = clone(fixtures.tokens);

  private me = fixtures.ME;
  private nextId = 1000;

  // --- Identity --------------------------------------------------------------

  async getMe(): Promise<User> {
    return latency(this.users.find((u) => u.id === this.me)!);
  }

  // --- Trips & membership ------------------------------------------------------

  async listTrips(): Promise<TripSummary[]> {
    return latency(
      this.trips.map((t) => ({
        id: t.id,
        name: t.name,
        coverPhotoUrl: t.coverPhotoUrl,
        accentColor: t.accentColor,
        status: t.status,
        startDate: t.startDate,
        endDate: t.endDate,
        memberCount: t.members.length,
        cities: this.citiesOf(t),
      })),
    );
  }

  async getTrip(tripId: string): Promise<Trip> {
    return latency(clone(this.mustFind(this.trips, tripId, 'trip')));
  }

  async createTrip(input: CreateTripInput): Promise<Trip> {
    const trip: Trip = {
      id: this.id('t'),
      name: input.name,
      coverPhotoUrl: null,
      accentColor: null,
      status: 'dreaming',
      startDate: input.startDate,
      endDate: input.endDate,
      baseCurrency: input.baseCurrency,
      members: [{ userId: this.me, role: 'leader', joinedAt: now() }],
      currentPlanId: null,
      createdAt: now(),
    };
    this.trips.push(trip);
    return latency(clone(trip));
  }

  async getUsers(tripId: string): Promise<User[]> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const ids = new Set(trip.members.map((m) => m.userId));
    return latency(clone(this.users.filter((u) => ids.has(u.id))));
  }

  async invite(tripId: string, email: string): Promise<Invite> {
    const invite: Invite = { id: this.id('inv'), tripId, email, invitedBy: this.me, status: 'pending', createdAt: now() };
    this.invites.push(invite);
    return latency(clone(invite));
  }

  async removeMember(tripId: string, userId: string): Promise<void> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    trip.members = trip.members.filter((m) => m.userId !== userId);
    return latency(undefined);
  }

  // --- Places & candidates -----------------------------------------------------

  async searchPlaces(query: string): Promise<Place[]> {
    const q = query.trim().toLowerCase();
    if (!q) return latency([]);
    return latency(
      clone(this.places.filter((p) => `${p.name} ${p.city} ${p.address}`.toLowerCase().includes(q))),
    );
  }

  async listCandidates(tripId: string): Promise<CandidateWithPlace[]> {
    return latency(
      clone(this.candidates.filter((c) => c.tripId === tripId).map((c) => this.withPlace(c))),
    );
  }

  async addCandidate(tripId: string, input: AddCandidateInput): Promise<CandidateWithPlace> {
    const candidate: Candidate = {
      id: this.id('c'),
      tripId,
      placeId: input.placeId,
      proposedBy: this.me,
      createdAt: now(),
      pitch: input.pitch,
      tags: input.tags,
      status: 'shortlisted',
    };
    this.candidates.push(candidate);
    return latency(clone(this.withPlace(candidate)));
  }

  private withPlace(candidate: Candidate): CandidateWithPlace {
    return { ...candidate, place: this.mustFind(this.places, candidate.placeId, 'place') };
  }

  // --- Plan ----------------------------------------------------------------------

  async getCurrentPlan(tripId: string): Promise<PlanDetail> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const plan = this.plans.find((p) => p.id === trip.currentPlanId);
    if (!plan) throw new ApiError(404, `trip ${tripId} has no current plan`);
    const days = this.days.filter((d) => d.planId === plan.id);
    const dayIds = new Set(days.map((d) => d.id));
    const stops = this.stops.filter((s) => dayIds.has(s.dayId));
    const stopIds = new Set(stops.map((s) => s.id));
    const legs = this.legs.filter((l) => stopIds.has(l.toStopId));
    const placeIds = new Set(stops.map((s) => s.placeId));
    return latency(
      clone({
        plan,
        days,
        stops,
        legs,
        dayFeasibility: this.dayFeasibility.filter((f) => dayIds.has(f.dayId)),
        places: this.places.filter((p) => placeIds.has(p.id)),
      }),
    );
  }

  async listPlanVersions(tripId: string): Promise<Plan[]> {
    return latency(clone(this.plans.filter((p) => p.tripId === tripId)));
  }

  // --- Content edits ---------------------------------------------------------------

  async updateStop(stopId: string, patch: StopPatch): Promise<Stop> {
    const stop = this.mustFind(this.stops, stopId, 'stop');
    this.applyPatch('stop', stop, patch);
    return latency(clone(stop));
  }

  async updateDay(dayId: string, patch: DayPatch): Promise<Day> {
    const day = this.mustFind(this.days, dayId, 'day');
    this.applyPatch('day', day, patch);
    return latency(clone(day));
  }

  async updateNotice(noticeId: string, patch: NoticePatch): Promise<Notice> {
    const notice = this.mustFind(this.notices, noticeId, 'notice');
    this.applyPatch('notice', notice, patch);
    return latency(clone(notice));
  }

  async getHistory(tripId: string): Promise<Edit[]> {
    return latency(
      clone(
        this.edits
          .filter((e) => e.tripId === tripId && e.status !== 'pending_review')
          .sort((a, b) => b.createdAt.localeCompare(a.createdAt)),
      ),
    );
  }

  async revertEdit(editId: string): Promise<void> {
    const edit = this.mustFind(this.edits, editId, 'edit');
    if (edit.status !== 'applied') throw new ApiError(409, 'only applied edits can be reverted');
    const pool: Record<string, { id: string }[]> = { stop: this.stops, day: this.days, notice: this.notices, candidate: this.candidates, trip: this.trips };
    const target = pool[edit.entity]?.find((x) => x.id === edit.entityId);
    if (target) (target as Record<string, unknown>)[edit.field] = clone(edit.oldValue);
    edit.status = 'reverted';
    return latency(undefined);
  }

  // --- Structural proposals -----------------------------------------------------------

  async listProposals(tripId: string): Promise<Proposal[]> {
    return latency(clone(this.proposals.filter((p) => p.tripId === tripId && p.status !== 'draft')));
  }

  async createProposal(tripId: string, input: CreateProposalInput): Promise<Proposal> {
    const isLeader = this.isLeader(tripId, this.me);
    const proposal: Proposal = {
      id: this.id('prop'),
      tripId,
      createdBy: this.me,
      source: { via: 'web' },
      title: input.title,
      rationale: input.rationale,
      changeSet: input.changeSet,
      route: input.route,
      // Leaders' own structural edits auto-apply (recorded for history)
      status: isLeader && input.route === 'leader_approval' ? 'applied' : 'pending',
      decidedBy: isLeader && input.route === 'leader_approval' ? { kind: 'leader', userId: this.me } : null,
      createdAt: now(),
    };
    this.proposals.push(proposal);
    return latency(clone(proposal));
  }

  async approveProposal(proposalId: string): Promise<Proposal> {
    const p = this.mustFind(this.proposals, proposalId, 'proposal');
    this.requireLeader(p.tripId);
    p.status = 'applied';
    p.decidedBy = { kind: 'leader', userId: this.me };
    return latency(clone(p));
  }

  async rejectProposal(proposalId: string): Promise<Proposal> {
    const p = this.mustFind(this.proposals, proposalId, 'proposal');
    this.requireLeader(p.tripId);
    p.status = 'rejected';
    p.decidedBy = { kind: 'leader', userId: this.me };
    return latency(clone(p));
  }

  async proposalToPoll(proposalId: string): Promise<Poll> {
    const p = this.mustFind(this.proposals, proposalId, 'proposal');
    this.requireLeader(p.tripId);
    p.route = 'poll';
    const poll: Poll = {
      id: this.id('poll'),
      tripId: p.tripId,
      createdBy: this.me,
      kind: 'plan_change',
      title: p.title,
      description: p.rationale,
      options: [
        { id: this.id('opt'), label: 'Adopt the change', proposalId: p.id },
        { id: this.id('opt'), label: 'Keep the current plan', proposalId: null },
      ],
      closesAt: daysFromNow(7),
      quorum: Math.ceil(this.mustFind(this.trips, p.tripId, 'trip').members.length / 2),
      allowMulti: false,
      status: 'open',
      votes: [],
    };
    p.decidedBy = { kind: 'poll', pollId: poll.id };
    this.polls.push(poll);
    return latency(clone(poll));
  }

  // --- Polls ------------------------------------------------------------------------

  async listPolls(tripId: string): Promise<Poll[]> {
    return latency(clone(this.polls.filter((p) => p.tripId === tripId)));
  }

  async createPoll(tripId: string, input: CreatePollInput): Promise<Poll> {
    const poll: Poll = {
      id: this.id('poll'),
      tripId,
      createdBy: this.me,
      kind: input.kind,
      title: input.title,
      description: input.description,
      options: input.options.map((o) => ({ id: this.id('opt'), label: o.label, proposalId: o.proposalId ?? null })),
      closesAt: input.closesAt,
      quorum: Math.ceil(this.mustFind(this.trips, tripId, 'trip').members.length / 2),
      allowMulti: input.allowMulti,
      status: 'open',
      votes: [],
    };
    this.polls.push(poll);
    return latency(clone(poll));
  }

  async vote(pollId: string, optionIds: string[]): Promise<Poll> {
    const poll = this.mustFind(this.polls, pollId, 'poll');
    if (poll.status !== 'open') throw new ApiError(409, 'poll is closed');
    poll.votes = poll.votes.filter((v) => v.userId !== this.me);
    for (const optionId of optionIds) poll.votes.push({ userId: this.me, optionId, at: now() });
    return latency(clone(poll));
  }

  async closePoll(pollId: string): Promise<Poll> {
    const poll = this.mustFind(this.polls, pollId, 'poll');
    this.requireLeader(poll.tripId);
    poll.status = poll.votes.length >= poll.quorum ? 'passed' : 'failed';
    return latency(clone(poll));
  }

  // --- AI airlock -----------------------------------------------------------------------

  async getReviewQueue(): Promise<ReviewItem[]> {
    return latency(clone(this.reviewItems));
  }

  async approveReviewItem(itemId: string): Promise<void> {
    const item = this.takeReviewItem(itemId);
    if (item.kind === 'edit') {
      const edit = this.mustFind(this.edits, item.edit.id, 'edit');
      edit.status = 'applied';
      const pool: Record<string, { id: string }[]> = { stop: this.stops, day: this.days, notice: this.notices, candidate: this.candidates, trip: this.trips };
      const target = pool[edit.entity]?.find((x) => x.id === edit.entityId);
      if (target) (target as Record<string, unknown>)[edit.field] = clone(edit.newValue);
    } else if (item.kind === 'proposal') {
      // Publishing, not applying — it still needs leader approval or a poll
      const p = this.mustFind(this.proposals, item.proposal.id, 'proposal');
      p.status = 'pending';
    } else if (item.kind === 'candidate') {
      this.candidates.push({ ...item.candidate, status: 'shortlisted' });
    }
    return latency(undefined);
  }

  async rejectReviewItem(itemId: string): Promise<void> {
    const item = this.takeReviewItem(itemId);
    if (item.kind === 'edit') this.mustFind(this.edits, item.edit.id, 'edit').status = 'rejected';
    if (item.kind === 'proposal') this.mustFind(this.proposals, item.proposal.id, 'proposal').status = 'rejected';
    return latency(undefined);
  }

  // --- Discussions ---------------------------------------------------------------------

  async listThreads(tripId: string): Promise<Thread[]> {
    return latency(clone(this.threads.filter((t) => t.tripId === tripId)));
  }

  async getComments(threadId: string): Promise<Comment[]> {
    return latency(clone(this.comments.filter((c) => c.threadId === threadId)));
  }

  async addComment(threadId: string, body: string): Promise<Comment> {
    const thread = this.mustFind(this.threads, threadId, 'thread');
    const comment: Comment = { id: this.id('cm'), threadId, author: this.me, body, createdAt: now(), reactions: [] };
    this.comments.push(comment);
    thread.commentCount += 1;
    thread.lastActivityAt = comment.createdAt;
    return latency(clone(comment));
  }

  // --- Ledger ---------------------------------------------------------------------------

  async getLedger(tripId: string): Promise<LedgerView> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const expenses = this.expenses.filter((e) => e.tripId === tripId);
    const settlements = this.settlements.filter((s) => s.tripId === tripId);
    return latency(clone(computeLedger(trip, expenses, settlements)));
  }

  async addExpense(tripId: string, input: AddExpenseInput): Promise<Expense> {
    const expense: Expense = {
      id: this.id('e'),
      tripId,
      paidBy: input.paidBy,
      amount: input.amount,
      currency: input.currency,
      fxRateToBase: input.currency === this.mustFind(this.trips, tripId, 'trip').baseCurrency ? 1 : mockFxRate(input.currency),
      category: input.category,
      split: input.split,
      note: input.note,
      receiptPhotoUrl: null,
      linkedStopId: input.linkedStopId ?? null,
      createdAt: now(),
    };
    this.expenses.push(expense);
    return latency(clone(expense));
  }

  async addSettlement(tripId: string, input: AddSettlementInput): Promise<Settlement> {
    const settlement: Settlement = { id: this.id('st'), tripId, ...input, settledAt: now() };
    this.settlements.push(settlement);
    return latency(clone(settlement));
  }

  // --- Notices -----------------------------------------------------------------------------

  async listNotices(tripId: string): Promise<Notice[]> {
    return latency(clone(this.notices.filter((n) => n.tripId === tripId)));
  }

  async createNotice(tripId: string, input: CreateNoticeInput): Promise<Notice> {
    const notice: Notice = {
      id: this.id('n'),
      tripId,
      category: input.category,
      title: input.title,
      body: input.body,
      sourceUrl: input.sourceUrl ?? null,
      pinned: false,
      checklistItems: (input.checklistItems ?? []).map((text) => ({ id: this.id('chk'), text, doneBy: [] })),
    };
    this.notices.push(notice);
    return latency(clone(notice));
  }

  async toggleChecklistItem(noticeId: string, itemId: string): Promise<Notice> {
    const notice = this.mustFind(this.notices, noticeId, 'notice');
    const item = notice.checklistItems.find((i) => i.id === itemId);
    if (!item) throw new ApiError(404, `checklist item ${itemId} not found`);
    item.doneBy = item.doneBy.includes(this.me) ? item.doneBy.filter((u) => u !== this.me) : [...item.doneBy, this.me];
    return latency(clone(notice));
  }

  // --- Tokens --------------------------------------------------------------------------------

  async listTokens(): Promise<ApiToken[]> {
    return latency(clone(this.tokens));
  }

  async createToken(input: CreateTokenInput): Promise<CreatedToken> {
    const n = this.nextId++;
    const plaintext = `itn_mock${n}fakeTokenForUiDevelopmentOnly`;
    const token: ApiToken = {
      id: this.id('tok'),
      name: input.name,
      prefix: plaintext.slice(0, 8),
      scopes: input.scopes,
      expiresAt: hoursFromNow(input.ttlHours),
      lastUsedAt: null,
      revokedAt: null,
      createdAt: now(),
    };
    this.tokens.push(token);
    return latency({ token: clone(token), plaintext });
  }

  async revokeToken(tokenId: string): Promise<void> {
    this.mustFind(this.tokens, tokenId, 'token').revokedAt = now();
    return latency(undefined);
  }

  // --- Internals -------------------------------------------------------------------------------

  private id(prefix: string): string {
    return `${prefix}-${this.nextId++}`;
  }

  private mustFind<T extends { id: string }>(pool: T[], id: string, kind: string): T {
    const found = pool.find((x) => x.id === id);
    if (!found) throw new ApiError(404, `${kind} ${id} not found`);
    return found;
  }

  private isLeader(tripId: string, userId: string): boolean {
    const trip = this.trips.find((t) => t.id === tripId);
    return trip?.members.some((m) => m.userId === userId && m.role === 'leader') ?? false;
  }

  private requireLeader(tripId: string): void {
    if (!this.isLeader(tripId, this.me)) throw new ApiError(403, 'leader role required');
  }

  private citiesOf(trip: Trip): string[] {
    const dayIds = new Set(this.days.filter((d) => d.planId === trip.currentPlanId).map((d) => d.id));
    const cities: string[] = [];
    for (const stop of this.stops.filter((s) => dayIds.has(s.dayId))) {
      const city = this.places.find((p) => p.id === stop.placeId)?.city;
      if (city && !cities.includes(city)) cities.push(city);
    }
    return cities;
  }

  /** Record field-level history, then apply — the content-edit contract (§3.3). */
  private applyPatch<T extends { id: string }>(entity: Edit['entity'], target: T, patch: Partial<T>): void {
    for (const [field, newValue] of Object.entries(patch)) {
      if (newValue === undefined) continue;
      const oldValue = (target as Record<string, unknown>)[field];
      if (JSON.stringify(oldValue) === JSON.stringify(newValue)) continue;
      this.edits.push({
        id: this.id('ed'),
        tripId: 't-japan26',
        entity,
        entityId: target.id,
        field,
        oldValue: clone(oldValue),
        newValue: clone(newValue),
        author: this.me,
        source: { via: 'web' },
        status: 'applied',
        createdAt: now(),
      });
      (target as Record<string, unknown>)[field] = clone(newValue);
    }
  }

  private takeReviewItem(itemId: string): ReviewItem {
    const idx = this.reviewItems.findIndex((i) => i.id === itemId);
    if (idx === -1) throw new ApiError(404, `review item ${itemId} not found`);
    return this.reviewItems.splice(idx, 1)[0];
  }
}

export class ApiError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

// --- Ledger math (mirrors what the backend will implement) ---------------------

function computeLedger(trip: Trip, expenses: Expense[], settlements: Settlement[]): LedgerView {
  const paid = new Map<string, number>();
  const owed = new Map<string, number>();
  for (const m of trip.members) {
    paid.set(m.userId, 0);
    owed.set(m.userId, 0);
  }

  for (const e of expenses) {
    const inBase = e.amount * e.fxRateToBase;
    paid.set(e.paidBy, (paid.get(e.paidBy) ?? 0) + inBase);
    for (const [userId, share] of shares(e, inBase)) {
      owed.set(userId, (owed.get(userId) ?? 0) + share);
    }
  }

  // A settlement moves money from debtor to creditor outside the expense pool
  const settled = new Map<string, number>();
  for (const s of settlements) {
    settled.set(s.fromUser, (settled.get(s.fromUser) ?? 0) + s.amount);
    settled.set(s.toUser, (settled.get(s.toUser) ?? 0) - s.amount);
  }

  const balances = trip.members.map((m) => {
    const p = round2(paid.get(m.userId) ?? 0);
    const o = round2(owed.get(m.userId) ?? 0);
    const net = round2(p - o + (settled.get(m.userId) ?? 0));
    return { userId: m.userId, paid: p, owed: o, net };
  });

  return { expenses, settlements, balances, suggestedTransfers: minCashFlow(balances) };
}

function shares(e: Expense, totalInBase: number): [string, number][] {
  if (e.split.kind === 'even') {
    const per = totalInBase / e.split.participantIds.length;
    return e.split.participantIds.map((id) => [id, per]);
  }
  if (e.split.kind === 'shares') {
    const totalWeight = e.split.participants.reduce((s, p) => s + p.weight, 0);
    return e.split.participants.map((p) => [p.userId, (totalInBase * p.weight) / totalWeight]);
  }
  return e.split.participants.map((p) => [p.userId, p.amount * e.fxRateToBase]);
}

/** Greedy min-cash-flow: repeatedly match the largest debtor with the largest creditor. */
function minCashFlow(balances: { userId: string; net: number }[]): LedgerView['suggestedTransfers'] {
  const creditors = balances.filter((b) => b.net > 0.01).map((b) => ({ ...b }));
  const debtors = balances.filter((b) => b.net < -0.01).map((b) => ({ ...b }));
  const transfers: LedgerView['suggestedTransfers'] = [];
  creditors.sort((a, b) => b.net - a.net);
  debtors.sort((a, b) => a.net - b.net);
  let ci = 0;
  let di = 0;
  while (ci < creditors.length && di < debtors.length) {
    const amount = round2(Math.min(creditors[ci].net, -debtors[di].net));
    if (amount > 0.01) transfers.push({ fromUser: debtors[di].userId, toUser: creditors[ci].userId, amount });
    creditors[ci].net = round2(creditors[ci].net - amount);
    debtors[di].net = round2(debtors[di].net + amount);
    if (creditors[ci].net <= 0.01) ci++;
    if (debtors[di].net >= -0.01) di++;
  }
  return transfers;
}

function mockFxRate(currency: string): number {
  const rates: Record<string, number> = { JPY: 0.0066, EUR: 1.16, GBP: 1.34, USD: 1 };
  return rates[currency] ?? 1;
}

// --- Small helpers ----------------------------------------------------------------

function clone<T>(value: T): T {
  return structuredClone(value);
}

function latency<T>(value: T): Promise<T> {
  const ms = 120 + (value === undefined ? 0 : 80);
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

function now(): string {
  return new Date().toISOString();
}

function hoursFromNow(h: number): string {
  return new Date(Date.now() + h * 3_600_000).toISOString();
}

function daysFromNow(d: number): string {
  return hoursFromNow(d * 24);
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}
