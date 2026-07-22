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
  ChangeOp,
  Comment,
  CreatedToken,
  Day,
  Edit,
  Expense,
  Invite,
  LedgerView,
  NewPlaceDraft,
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
  // Search-only catalog: real places not yet in the plan/shortlist. Kept apart
  // from `places` (which is only what stops reference) so it's obvious these are
  // discoverable-but-not-adopted until a proposal pulls one in.
  private catalog: Place[] = clone(fixtures.catalog);
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
      stopKindLabels: null,
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
    // The trip's own places first, then the wider catalog; dedupe by id so a
    // place already in the plan isn't offered twice.
    const seen = new Set<string>();
    const hits: Place[] = [];
    for (const p of [...this.places, ...this.catalog]) {
      if (seen.has(p.id)) continue;
      if (`${p.name} ${p.city} ${p.address}`.toLowerCase().includes(q)) {
        seen.add(p.id);
        hits.push(p);
      }
    }
    return latency(clone(hits));
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
      status: 'pending',
      decidedBy: null,
      rejectionReason: null,
      createdAt: now(),
    };
    this.proposals.push(proposal);
    // A leader's own structural edit via the fast path applies immediately,
    // recorded as an auto-approved proposal so history stays complete (§3.3).
    if (isLeader && input.route === 'leader_approval') this.applyProposal(proposal);
    return latency(clone(proposal));
  }

  async approveProposal(proposalId: string): Promise<Proposal> {
    const p = this.mustFind(this.proposals, proposalId, 'proposal');
    this.requireLeader(p.tripId);
    this.applyProposal(p);
    return latency(clone(p));
  }

  async rejectProposal(proposalId: string, reason: string): Promise<Proposal> {
    const p = this.mustFind(this.proposals, proposalId, 'proposal');
    this.requireLeader(p.tripId);
    if (!reason.trim()) throw new ApiError(400, 'a rejection reason is required');
    p.status = 'rejected';
    p.decidedBy = { kind: 'leader', userId: this.me };
    p.rejectionReason = reason.trim();
    return latency(clone(p));
  }

  /**
   * Apply a structural proposal: mint the next plan version, re-parent the live
   * day/stop set onto it, run each ChangeOp, then re-run feasibility. The trip's
   * currentPlanId advances so the Plan tab visibly changes; the prior Plan row
   * stays in history for rollback. Idempotent — re-applying is a no-op.
   */
  private applyProposal(p: Proposal): Plan {
    const trip = this.mustFind(this.trips, p.tripId, 'trip');
    const oldPlan = this.plans.find((pl) => pl.id === trip.currentPlanId);
    if (p.status === 'applied' || !oldPlan) return oldPlan!;
    const nextVersion = Math.max(...this.plans.filter((pl) => pl.tripId === p.tripId).map((pl) => pl.version)) + 1;
    const newPlan: Plan = { id: this.id('plan'), tripId: p.tripId, version: nextVersion, createdFromProposalId: p.id, createdAt: now() };
    this.plans.push(newPlan);
    for (const d of this.days.filter((d) => d.planId === oldPlan.id)) d.planId = newPlan.id;
    for (const op of p.changeSet.ops) this.applyOp(op, newPlan.id);
    this.recomputeFeasibility(newPlan.id);
    trip.currentPlanId = newPlan.id;
    p.status = 'applied';
    p.decidedBy = p.decidedBy ?? { kind: 'leader', userId: this.me };
    return newPlan;
  }

  private applyOp(op: ChangeOp, planId: string): void {
    if (op.op === 'remove_stop') {
      this.stops = this.stops.filter((s) => s.id !== op.stopId);
    } else if (op.op === 'move_stop') {
      const s = this.stops.find((x) => x.id === op.stopId);
      if (s) {
        s.dayId = op.toDayId;
        s.seq = op.seq;
        this.resequence(op.toDayId);
      }
    } else if (op.op === 'add_stop') {
      this.stops.push({ id: this.id('s'), dayId: op.dayId, seq: op.seq, placeId: op.placeId, stopKind: op.stopKind, plannedArrival: '12:00', durationMin: 60, booking: null, notes: '' });
      this.resequence(op.dayId);
    } else if (op.op === 'add_place_stop') {
      // Materialise the drafted place first (Phase B geocodes it; the mock
      // drops it near the day's other stops so it lands on the map), then add
      // its stop. The draft's note seeds the stop's notes.
      const place = this.materialiseDraft(op.draft, op.dayId);
      this.places.push(place);
      this.stops.push({ id: this.id('s'), dayId: op.dayId, seq: op.seq, placeId: place.id, stopKind: op.stopKind, plannedArrival: '12:00', durationMin: 60, booking: null, notes: op.draft.note });
      this.resequence(op.dayId);
    } else if (op.op === 'reorder') {
      op.stopIdsInOrder.forEach((sid, i) => {
        const s = this.stops.find((x) => x.id === sid);
        if (s) s.seq = i + 1;
      });
    } else if (op.op === 'swap_place') {
      const s = this.stops.find((x) => x.id === op.stopId);
      if (s) s.placeId = op.newPlaceId;
    } else if (op.op === 'add_day') {
      this.days.push({ id: this.id('d'), planId, date: op.date, cityHint: op.cityHint, tz: 'Asia/Tokyo', windowStart: '09:00', windowEnd: '21:00' });
    } else if (op.op === 'remove_day') {
      this.days = this.days.filter((d) => d.id !== op.dayId);
      this.stops = this.stops.filter((s) => s.dayId !== op.dayId);
    }
  }

  /**
   * Turn a NewPlaceDraft into a full Place. When the drafter picked the spot off
   * the map/search its real `lat`/`lng` ride along and are used verbatim.
   * Otherwise the backend would geocode `name`; the mock drops it a hair off the
   * day's centroid (so it renders on the map, never at 0,0). Provider fields stay
   * empty — it's a user-typed spot. Website carries the drafted URL when valid.
   */
  private materialiseDraft(draft: NewPlaceDraft, dayId: string): Place {
    const day = this.days.find((d) => d.id === dayId);
    const dayStops = this.stops.filter((s) => s.dayId === dayId);
    const pts = dayStops.map((s) => this.places.find((p) => p.id === s.placeId)).filter((p): p is Place => !!p);
    const centroidLat = pts.length ? pts.reduce((n, p) => n + p.lat, 0) / pts.length + 0.004 : 35.68;
    const centroidLng = pts.length ? pts.reduce((n, p) => n + p.lng, 0) / pts.length + 0.004 : 139.76;
    const lat = draft.lat ?? centroidLat;
    const lng = draft.lng ?? centroidLng;
    const isUrl = /^https?:\/\//i.test(draft.url ?? '');
    return {
      id: this.id('p'),
      name: draft.name,
      kind: draft.kind,
      lat,
      lng,
      tz: day?.tz ?? 'Asia/Tokyo',
      countryCode: 'JP',
      adminArea: draft.city,
      city: draft.city,
      address: draft.city,
      externalRef: null, // typed by a member, not resolved against a catalog yet
      website: isUrl ? draft.url : null,
      phone: null,
      rating: null,
      priceLevel: null,
      openingHours: null,
      photoUrls: [],
    };
  }

  /** Close gaps in a day's seq numbers after a move/insert/remove. */
  private resequence(dayId: string): void {
    this.stops
      .filter((s) => s.dayId === dayId)
      .sort((a, b) => a.seq - b.seq)
      .forEach((s, i) => {
        s.seq = i + 1;
      });
  }

  /** Cheap stand-in for the Phase-B feasibility engine (§5): time used vs. the
      day window, banded at the 85%/100% thresholds. Notes are regenerated. */
  private recomputeFeasibility(planId: string): void {
    for (const day of this.days.filter((d) => d.planId === planId)) {
      const dayStops = this.stops.filter((s) => s.dayId === day.id);
      const stopIds = new Set(dayStops.map((s) => s.id));
      const visitMin = dayStops.reduce((n, s) => n + s.durationMin, 0);
      const legMin = this.legs.filter((l) => stopIds.has(l.toStopId)).reduce((n, l) => n + l.durationMin, 0);
      const usedMin = visitMin + legMin;
      const windowMin = minutesBetween(day.windowStart, day.windowEnd);
      const pct = usedMin / windowMin;
      const feasibility = pct > 1 ? 'unreasonable' : pct >= 0.85 ? 'tight' : 'ok';
      const existing = this.dayFeasibility.find((f) => f.dayId === day.id);
      const notes = feasibility === 'ok' ? [] : [`${Math.round(pct * 100)}% of the day window used after the last change.`];
      if (existing) {
        existing.feasibility = feasibility;
        existing.usedMin = usedMin;
        existing.windowMin = windowMin;
        existing.notes = notes;
      } else {
        this.dayFeasibility.push({ dayId: day.id, feasibility, usedMin, windowMin, notes });
      }
    }
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
      opensAt: null,
      closesAt: daysFromNow(7),
      quorum: Math.ceil(this.mustFind(this.trips, p.tripId, 'trip').members.length / 2),
      allowMulti: false,
      status: 'open',
      resolutionNote: null,
      votes: [],
    };
    p.status = 'pending';
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
      opensAt: null,
      closesAt: input.closesAt,
      quorum: Math.ceil(this.mustFind(this.trips, tripId, 'trip').members.length / 2),
      allowMulti: input.allowMulti,
      status: 'open',
      resolutionNote: null,
      votes: [],
    };
    this.polls.push(poll);
    return latency(clone(poll));
  }

  async openPoll(pollId: string): Promise<Poll> {
    const poll = this.mustFind(this.polls, pollId, 'poll');
    if (poll.status !== 'draft' && poll.status !== 'scheduled') throw new ApiError(409, 'only a draft or scheduled poll can be opened');
    // Only a leader or the poll's author may publish it.
    if (poll.createdBy !== this.me) this.requireLeader(poll.tripId);
    poll.status = 'open';
    poll.opensAt = null;
    return latency(clone(poll));
  }

  async vote(pollId: string, optionIds: string[]): Promise<Poll> {
    const poll = this.mustFind(this.polls, pollId, 'poll');
    if (poll.status !== 'open') throw new ApiError(409, 'poll is not open for voting');
    poll.votes = poll.votes.filter((v) => v.userId !== this.me);
    for (const optionId of optionIds) poll.votes.push({ userId: this.me, optionId, at: now() });
    return latency(clone(poll));
  }

  async closePoll(pollId: string): Promise<Poll> {
    const poll = this.mustFind(this.polls, pollId, 'poll');
    this.requireLeader(poll.tripId);
    // Below quorum → expired (no decision). At/above → passed or failed by winner.
    if (poll.votes.length < poll.quorum) {
      poll.status = 'expired';
      poll.resolutionNote = 'Closed below quorum — no decision recorded.';
      return latency(clone(poll));
    }
    const counts = new Map<string, number>();
    for (const v of poll.votes) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
    const winner = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
    const winningOption = poll.options.find((o) => o.id === winner);
    // A plan_change poll "passes" only when the adopt option (with a proposal) wins.
    if (poll.kind === 'plan_change') {
      if (winningOption?.proposalId) {
        poll.status = 'passed';
        const proposal = this.proposals.find((p) => p.id === winningOption.proposalId);
        if (proposal && proposal.status !== 'applied') this.applyProposal(proposal);
      } else {
        poll.status = 'failed';
      }
    } else {
      poll.status = 'passed';
    }
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

  async toggleReaction(commentId: string, emoji: string): Promise<Comment> {
    const comment = this.mustFind(this.comments, commentId, 'comment');
    const existing = comment.reactions.find((r) => r.emoji === emoji);
    if (existing) {
      existing.userIds = existing.userIds.includes(this.me)
        ? existing.userIds.filter((u) => u !== this.me)
        : [...existing.userIds, this.me];
      comment.reactions = comment.reactions.filter((r) => r.userIds.length > 0);
    } else {
      comment.reactions.push({ emoji, userIds: [this.me] });
    }
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

/** Minutes between two "HH:MM" local times on the same day. */
function minutesBetween(start: string, end: string): number {
  const [sh, sm] = start.split(':').map(Number);
  const [eh, em] = end.split(':').map(Number);
  return eh * 60 + em - (sh * 60 + sm);
}
