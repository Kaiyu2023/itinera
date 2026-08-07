import type {
  AddCandidateInput,
  AddExpenseInput,
  AddSettlementInput,
  ApiClient,
  CandidatePlaceInput,
  CreateNoticeInput,
  CreatePollInput,
  CreateProposalInput,
  CreateThreadInput,
  CreateTokenInput,
  CreateTripInput,
  DayPatch,
  ExpensePatch,
  InitializePlanInput,
  NoticePatch,
  StopPatch,
  UpdateCandidateInput,
} from '../client';
import type {
  ApiToken,
  Candidate,
  CandidateDisposition,
  CandidateWithPlace,
  ChangeOp,
  ChangeSet,
  Comment,
  ContentHistoryEdit,
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
  ThreadAnchor,
  Trip,
  TripStatus,
  TripSummary,
  User,
} from '../types';
import * as fixtures from './fixtures';

const CONTENT_HISTORY_SAFETY_LIMIT = 1_000;
const CONTENT_HISTORY_BYTE_LIMIT = 4 * 1_024 * 1_024;

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
  /** Candidate-owned copies stay joinable without becoming duplicate search hits. */
  private candidateSnapshotIds = new Set<string>();
  private plans: Plan[] = clone(fixtures.planVersions);
  private days: Day[] = clone(fixtures.days);
  private stops: Stop[] = clone(fixtures.stops);
  private legs = clone(fixtures.legs);
  private dayFeasibility = clone(fixtures.dayFeasibility);
  private proposals: Proposal[] = clone(fixtures.proposals);
  private polls: Poll[] = freshenActivePollDeadlines(clone(fixtures.polls));
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

  async setTripStatus(tripId: string, status: TripStatus): Promise<Trip> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    // No ordering check on purpose: bookings fall through and dates slip, so
    // `booked` → `planning` has to be as cheap as the other direction.
    this.applyPatch('trip', trip, { status }, trip.id);
    return latency(clone(trip));
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
    const invite: Invite = {
      id: this.id('inv'),
      tripId,
      email,
      invitedBy: this.me,
      status: 'pending',
      createdAt: now(),
    };
    this.invites.push(invite);
    return latency(clone(invite));
  }

  async removeMember(tripId: string, userId: string): Promise<void> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    trip.members = trip.members.filter((m) => m.userId !== userId);
    return latency(undefined);
  }

  // --- Places & candidates -----------------------------------------------------

  async searchPlaces(tripId: string, query: string): Promise<Place[]> {
    const q = query.trim().toLowerCase();
    const searchable = this.searchablePlacesForTrip(tripId);
    if (!q) return latency([]);
    return latency(
      clone(searchable.filter((place) => `${place.name} ${place.city} ${place.address}`.toLowerCase().includes(q))),
    );
  }

  async listCandidates(tripId: string): Promise<CandidateWithPlace[]> {
    return latency(clone(this.candidates.filter((c) => c.tripId === tripId).map((c) => this.withPlace(c))));
  }

  async addCandidate(tripId: string, input: AddCandidateInput): Promise<CandidateWithPlace> {
    const placeInput = this.normaliseCandidatePlace(input.place);
    this.validateCandidateInput({ ...input, place: placeInput });
    const source = input.sourcePlaceId
      ? (this.searchablePlacesForTrip(tripId).find((p) => p.id === input.sourcePlaceId) ??
        (() => {
          throw new ApiError(404, `place ${input.sourcePlaceId} not found`);
        })())
      : null;
    // Candidate place copy-on-write is deliberate. A catalog result may also
    // be used by a planned stop, so applying the member's guide/contact edits
    // to that shared object would silently rewrite the itinerary. Each idea
    // owns a materialised copy while retaining provider facts from its source.
    const place = this.materialiseCandidatePlace(placeInput, source, tripId);
    this.places.push(place);
    this.candidateSnapshotIds.add(place.id);
    const candidate: Candidate = {
      id: this.id('c'),
      tripId,
      sourcePlaceId: input.sourcePlaceId,
      placeId: place.id,
      proposedBy: this.me,
      createdAt: now(),
      pitch: input.pitch,
      tags: input.tags,
      status: 'shortlisted',
    };
    this.candidates.push(candidate);
    return latency(clone(this.withPlace(candidate)));
  }

  async updateCandidate(tripId: string, candidateId: string, input: UpdateCandidateInput): Promise<CandidateWithPlace> {
    const placeInput = this.normaliseCandidatePlace(input.place);
    this.validateCandidateInput({ ...input, place: placeInput });
    const candidate = this.mustFindForTrip(this.candidates, tripId, candidateId, 'candidate');
    const currentPlace = this.mustFind(this.places, candidate.placeId, 'place');
    // Fork on every edit rather than guessing whether the current place is
    // shared. This keeps an applied-plan place, another candidate, and catalog
    // search results immutable even when their ids used to match this idea.
    const place = this.materialiseCandidatePlace(placeInput, currentPlace, tripId);
    this.places.push(place);
    this.candidateSnapshotIds.add(place.id);
    this.recordEdit('candidate', candidate.id, 'place', currentPlace, place, tripId);
    candidate.placeId = place.id;
    this.applyPatch('candidate', candidate, { pitch: input.pitch, tags: clone(input.tags) }, tripId);
    return latency(clone(this.withPlace(candidate)));
  }

  async setCandidateStatus(
    tripId: string,
    candidateId: string,
    status: CandidateDisposition,
  ): Promise<CandidateWithPlace> {
    const candidate = this.mustFindForTrip(this.candidates, tripId, candidateId, 'candidate');
    // `in_plan` is a consequence of an applied proposal, not something a member
    // can assert — the shortlist controls only ever shortlist or reject.
    if (candidate.status === 'in_plan') throw new ApiError(409, 'this candidate is already in the plan');
    this.applyPatch('candidate', candidate, { status }, tripId);
    return latency(clone(this.withPlace(candidate)));
  }

  private withPlace(candidate: Candidate): CandidateWithPlace {
    return { ...candidate, place: this.mustFind(this.places, candidate.placeId, 'place') };
  }

  private validateCandidateInput(input: Pick<AddCandidateInput, 'place' | 'pitch' | 'tags'>): void {
    if (!input.place.name.trim()) throw new ApiError(400, 'place name is required');
    if (!input.place.city.trim()) throw new ApiError(400, 'place city is required');
    if (!input.pitch.trim()) throw new ApiError(400, 'candidate pitch is required');
    if (input.place.guide && (!input.place.guide.summary.trim() || !input.place.guide.intro.trim())) {
      throw new ApiError(400, 'guide summary and introduction are required when guide content is provided');
    }
    if (input.place.guide?.activityIdeas.some((idea) => !idea.title.trim())) {
      throw new ApiError(400, 'activity idea title is required');
    }
  }

  private normaliseCandidatePlace(input: CandidatePlaceInput): CandidatePlaceInput {
    const guide = input.guide
      ? {
          summary: input.guide.summary.trim(),
          intro: input.guide.intro.trim(),
          activityIdeas: input.guide.activityIdeas.map((idea) => ({
            title: idea.title.trim(),
            ...(idea.details?.trim() ? { details: idea.details.trim() } : {}),
          })),
          practicalTips: input.guide.practicalTips.map((tip) => tip.trim()).filter(Boolean),
        }
      : null;
    return {
      name: input.name.trim(),
      kind: input.kind,
      city: input.city.trim(),
      address: input.address.trim(),
      website: input.website?.trim() || null,
      phone: input.phone?.trim() || null,
      openingHours: input.openingHours.map((hours) => hours.trim()).filter(Boolean),
      photoUrls: input.photoUrls.map((url) => url.trim()).filter(Boolean),
      guide,
    };
  }

  private materialiseCandidatePlace(input: CandidatePlaceInput, source: Place | null, tripId: string): Place {
    const seed = source ?? this.placeSeedForCity(tripId, input.city);
    return {
      id: this.id('p-candidate'),
      name: input.name,
      kind: input.kind,
      lat: seed?.lat ?? 0,
      lng: seed?.lng ?? 0,
      tz: seed?.tz ?? 'UTC',
      countryCode: seed?.countryCode ?? '',
      adminArea: seed?.adminArea ?? '',
      city: input.city,
      address: input.address,
      externalRef: clone(source?.externalRef ?? null),
      website: input.website,
      phone: input.phone,
      rating: source?.rating ?? null,
      priceLevel: source?.priceLevel ?? null,
      openingHours: input.openingHours.length ? { weekdayText: clone(input.openingHours) } : null,
      photoUrls: clone(input.photoUrls),
      guide: clone(input.guide),
    };
  }

  private placeSeedForCity(tripId: string, city: string): Place | null {
    const key = city.trim().toLocaleLowerCase();
    return this.searchablePlacesForTrip(tripId).find((place) => place.city.trim().toLocaleLowerCase() === key) ?? null;
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

  async initializePlan(tripId: string, input: InitializePlanInput): Promise<PlanDetail> {
    const trip = this.mustFind(this.trips, tripId, 'trip');

    // The action is safe to retry. This matters in the Candidates tab where a
    // slow current-plan read and the first "Propose for a day" click can cross
    // in flight; neither a double click nor a network retry may mint Plan v2.
    if (trip.currentPlanId) return this.getCurrentPlan(tripId);

    const candidate = this.candidates.find(
      (item) => item.tripId === tripId && item.placeId === input.anchorPlaceId && item.status === 'shortlisted',
    );
    if (!candidate) throw new ApiError(404, `shortlisted place ${input.anchorPlaceId} not found for trip ${tripId}`);
    const anchor = this.mustFind(this.places, candidate.placeId, 'place');

    const plan: Plan = {
      id: this.id('plan'),
      tripId,
      version: 1,
      createdFromProposalId: null,
      createdAt: now(),
    };
    this.plans.push(plan);
    trip.currentPlanId = plan.id;
    trip.status = 'planning';

    for (const date of datesInclusive(trip.startDate, trip.endDate)) {
      const day: Day = {
        id: this.id('d'),
        planId: plan.id,
        date,
        cityHint: anchor.city,
        tz: anchor.tz,
        windowStart: '09:00',
        windowEnd: '21:00',
      };
      this.days.push(day);
      this.dayFeasibility.push({
        dayId: day.id,
        feasibility: 'ok',
        usedMin: 0,
        windowMin: minutesBetween(day.windowStart, day.windowEnd),
        notes: [],
      });
    }

    return this.getCurrentPlan(tripId);
  }

  async listPlanVersions(tripId: string): Promise<Plan[]> {
    return latency(clone(this.plans.filter((p) => p.tripId === tripId)));
  }

  // --- Content edits ---------------------------------------------------------------

  async updateStop(tripId: string, stopId: string, patch: StopPatch): Promise<Stop> {
    const stop = this.mustFindStopForTrip(tripId, stopId);
    const currentLedgerEntryId = stop.booking?.ledgerEntryId ?? null;
    if (patch.booking === null && currentLedgerEntryId !== null) {
      throw new ApiError(409, 'unlink the ledger expense before removing its booking');
    }
    const safePatch: Record<string, unknown> = { ...patch };
    if (patch.booking !== undefined && patch.booking !== null) {
      safePatch.booking = { ...patch.booking, ledgerEntryId: currentLedgerEntryId };
    }
    this.applyPatch('stop', stop, safePatch, tripId);
    return latency(clone(stop));
  }

  async updateDay(tripId: string, dayId: string, patch: DayPatch): Promise<Day> {
    const day = this.mustFindDayForTrip(tripId, dayId);
    this.applyPatch('day', day, patch, tripId);
    return latency(clone(day));
  }

  async updateNotice(tripId: string, noticeId: string, patch: NoticePatch): Promise<Notice> {
    const notice = this.mustFindForTrip(this.notices, tripId, noticeId, 'notice');
    if (notice.createdBy !== this.me && !this.isLeader(notice.tripId, this.me)) {
      throw new ApiError(403, 'only the notice author or a trip leader may manage this notice');
    }
    this.applyPatch('notice', notice, patch, tripId);
    return latency(clone(notice));
  }

  async getHistory(tripId: string): Promise<ContentHistoryEdit[]> {
    this.requireMember(tripId);
    const tripEdits = this.edits.filter((edit) => edit.tripId === tripId);
    assertContentHistoryStorageBudget(tripEdits);
    const visible = visibleContentHistory(tripEdits).sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    assertContentHistoryResponseBudget(visible);
    return latency(clone(visible));
  }

  async revertEdit(tripId: string, editId: string): Promise<void> {
    this.requireEditor(tripId);
    const tripEdits = this.edits.filter((edit) => edit.tripId === tripId);
    assertContentHistoryStorageBudget(tripEdits);
    const edit = this.mustFindForTrip(
      tripEdits.filter((item) => item.status === 'applied' || item.status === 'reverted'),
      tripId,
      editId,
      'edit',
    );
    if (edit.status === 'reverted') return latency(undefined);
    if (tripEdits.length >= CONTENT_HISTORY_SAFETY_LIMIT) {
      throw new ApiError(409, 'this history operation exceeds the current safe processing limit');
    }
    if (
      edit.entity === 'candidate' &&
      edit.field === 'status' &&
      (edit.oldValue === 'in_plan' || edit.newValue === 'in_plan')
    ) {
      throw new ApiError(409, 'proposal-owned in-plan state cannot be reverted as content');
    }

    let target: Record<string, unknown>;
    let candidatePlaceRepoint: { candidate: Candidate; previousPlaceId: string } | null = null;
    if (edit.entity === 'trip' && edit.entityId === tripId && edit.field === 'status') {
      target = this.mustFind(this.trips, edit.entityId, 'trip') as unknown as Record<string, unknown>;
    } else if (edit.entity === 'candidate' && ['status', 'pitch', 'tags'].includes(edit.field)) {
      target = this.mustFindForTrip(this.candidates, tripId, edit.entityId, 'candidate') as unknown as Record<
        string,
        unknown
      >;
    } else if (edit.entity === 'candidate' && edit.field === 'place') {
      const candidate = this.mustFindForTrip(this.candidates, tripId, edit.entityId, 'candidate');
      const currentPlace = this.mustFind(this.places, candidate.placeId, 'place');
      if (JSON.stringify(currentPlace) !== JSON.stringify(edit.newValue)) {
        throw new ApiError(409, 'the edited field has changed since this history entry was applied');
      }
      const previous = edit.oldValue as Place;
      const previousSnapshot = this.mustFind(this.places, previous.id, 'place');
      if (previous.id === currentPlace.id || JSON.stringify(previousSnapshot) !== JSON.stringify(previous)) {
        throw new ApiError(409, 'the stored edit does not identify valid candidate place snapshots');
      }
      candidatePlaceRepoint = { candidate, previousPlaceId: previous.id };
      target = { place: currentPlace };
    } else if (edit.entity === 'day' && ['windowStart', 'windowEnd', 'cityHint'].includes(edit.field)) {
      target = this.mustFindCurrentDayForTrip(tripId, edit.entityId) as unknown as Record<string, unknown>;
    } else if (edit.entity === 'stop' && ['plannedArrival', 'durationMin', 'notes', 'booking'].includes(edit.field)) {
      target = this.mustFindCurrentStopForTrip(tripId, edit.entityId) as unknown as Record<string, unknown>;
    } else {
      throw new ApiError(409, 'this edit target cannot be reverted safely');
    }
    let actualBefore = clone(target[edit.field]);
    let actualAfter = clone(edit.oldValue);
    if (edit.entity === 'stop' && edit.field === 'booking') {
      const ledgerId = (value: unknown): string | null =>
        value && typeof value === 'object' && 'ledgerEntryId' in value
          ? ((value as { ledgerEntryId: string | null }).ledgerEntryId ?? null)
          : null;
      if (ledgerId(edit.oldValue) !== ledgerId(edit.newValue)) {
        throw new ApiError(500, 'stored booking history attempted to edit a server-owned ledger link');
      }
      const withoutLedgerId = (value: unknown): unknown =>
        value && typeof value === 'object' ? { ...(value as object), ledgerEntryId: null } : value;
      if (JSON.stringify(withoutLedgerId(actualBefore)) !== JSON.stringify(withoutLedgerId(edit.newValue))) {
        throw new ApiError(409, 'the edited field has changed since this history entry was applied');
      }
      const currentLedgerId = ledgerId(actualBefore);
      if (actualAfter === null && currentLedgerId !== null) {
        throw new ApiError(409, 'unlink the ledger expense before reverting away its booking');
      }
      if (actualAfter && typeof actualAfter === 'object') {
        actualAfter = { ...(actualAfter as object), ledgerEntryId: currentLedgerId };
      }
    } else if (edit.entity !== 'candidate' || edit.field !== 'place') {
      if (JSON.stringify(actualBefore) !== JSON.stringify(edit.newValue)) {
        throw new ApiError(409, 'the edited field has changed since this history entry was applied');
      }
    }
    const revertedAt = now();
    const compensationId = `ed-${this.nextId}`;
    const reverted: Edit = {
      ...clone(edit),
      status: 'reverted',
      revertedBy: this.me,
      revertedAt,
      revertEditId: compensationId,
    };
    const compensation: Edit = {
      id: compensationId,
      tripId,
      entity: edit.entity,
      entityId: edit.entityId,
      field: edit.field,
      oldValue: clone(actualBefore),
      newValue: clone(actualAfter),
      author: this.me,
      source: { via: 'web' },
      status: 'applied',
      createdAt: revertedAt,
      revertedBy: null,
      revertedAt: null,
      revertEditId: null,
      revertsEditId: edit.id,
    };
    const projected = [...tripEdits.filter((item) => item.id !== edit.id), reverted, compensation];
    assertContentHistoryStorageBudget(projected);
    assertContentHistoryResponseBudget(visibleContentHistory(projected));
    this.nextId += 1;
    if (candidatePlaceRepoint) {
      candidatePlaceRepoint.candidate.placeId = candidatePlaceRepoint.previousPlaceId;
    } else {
      target[edit.field] = clone(actualAfter);
    }
    Object.assign(edit, reverted);
    this.edits.push(compensation);
    return latency(undefined);
  }

  // --- Structural proposals -----------------------------------------------------------

  async listProposals(tripId: string): Promise<Proposal[]> {
    this.requireMember(tripId);
    const proposals = this.proposals
      .filter((proposal) => proposal.tripId === tripId && proposal.status !== 'draft')
      .sort((left, right) => right.createdAt.localeCompare(left.createdAt) || right.id.localeCompare(left.id));
    return latency(clone(proposals));
  }

  async createProposal(tripId: string, input: CreateProposalInput): Promise<Proposal> {
    this.requireEditor(tripId);
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const currentPlan = this.plans.find((plan) => plan.id === trip.currentPlanId);
    if (!currentPlan) throw new ApiError(409, 'trip has no current plan to change');
    // Validate every nested reference before persisting the Proposal or opening
    // its poll. A child id is not authority, and a rejected command must leave
    // no partially-created governance records behind.
    this.validateChangeSet(tripId, currentPlan, input.changeSet);
    const isLeader = this.isLeader(tripId, this.me);
    const proposal: Proposal = {
      id: this.id('prop'),
      tripId,
      createdBy: this.me,
      source: { via: 'web' },
      title: input.title,
      rationale: input.rationale,
      changeSet: clone(input.changeSet),
      route: input.route,
      status: 'pending',
      decidedBy: null,
      rejectionReason: null,
      createdAt: now(),
    };
    this.proposals.push(proposal);
    if (input.route === 'poll') {
      // A poll-routed proposal opens its plan_change poll straight away so the
      // group can start voting — otherwise it would sit pending forever.
      this.openPlanChangePoll(proposal);
    } else if (isLeader) {
      // A leader's own structural edit via the fast path applies immediately,
      // recorded as an auto-approved proposal so history stays complete (§3.3).
      this.applyProposal(proposal);
    }
    return latency(clone(proposal));
  }

  async approveProposal(tripId: string, proposalId: string): Promise<Proposal> {
    this.requireLeader(tripId);
    const p = this.mustFindForTrip(this.proposals, tripId, proposalId, 'proposal');
    if (p.route !== 'leader_approval') throw new ApiError(409, 'poll-routed proposals are decided by their poll');
    if (p.status === 'applied') return latency(clone(p));
    if (p.status !== 'pending') throw new ApiError(409, 'proposal cannot be approved in its current state');
    this.applyProposal(p);
    return latency(clone(p));
  }

  async rejectProposal(tripId: string, proposalId: string, reason: string): Promise<Proposal> {
    this.requireLeader(tripId);
    const p = this.mustFindForTrip(this.proposals, tripId, proposalId, 'proposal');
    if (p.route !== 'leader_approval') throw new ApiError(409, 'poll-routed proposals are decided by their poll');
    if (!reason.trim()) throw new ApiError(400, 'a rejection reason is required');
    if (p.status === 'rejected') return latency(clone(p));
    if (p.status !== 'pending') throw new ApiError(409, 'proposal cannot be rejected in its current state');
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
    // Re-applying a proposal is intentionally idempotent, but every first
    // application must still be based on the live plan. Keep this guard here,
    // at the mutation boundary, so leader approval, poll closure, and any
    // future direct application path all get the same optimistic-lock check.
    if (p.status === 'applied' && oldPlan) return oldPlan;
    if (!oldPlan) throw new ApiError(409, 'trip has no current plan to change');
    if (p.changeSet.basePlanVersion !== oldPlan.version) {
      p.status = 'stale';
      throw new ApiError(
        409,
        `proposal is based on plan v${p.changeSet.basePlanVersion}; current plan is v${oldPlan.version}`,
      );
    }
    // Re-run the full preflight at the write boundary. A proposal can wait for
    // a leader or a poll, so references that were valid at submission time may
    // no longer describe the live plan when the decision arrives.
    this.validateChangeSet(p.tripId, oldPlan, p.changeSet);
    const nextVersion = Math.max(...this.plans.filter((pl) => pl.tripId === p.tripId).map((pl) => pl.version)) + 1;
    const newPlan: Plan = {
      id: this.id('plan'),
      tripId: p.tripId,
      version: nextVersion,
      createdFromProposalId: p.id,
      createdAt: now(),
    };
    this.plans.push(newPlan);
    for (const d of this.days.filter((d) => d.planId === oldPlan.id)) d.planId = newPlan.id;
    for (const op of p.changeSet.ops) this.applyOp(op, newPlan.id, p.tripId);
    // Candidate state follows the structural outcome, never the proposal's
    // mere existence. This also makes the first adopted idea leave "Ideas to
    // consider" only after its leader/poll route actually applies.
    const currentDayIds = new Set(this.days.filter((day) => day.planId === newPlan.id).map((day) => day.id));
    const adoptedPlaceIds = new Set(
      this.stops.filter((stop) => currentDayIds.has(stop.dayId)).map((stop) => stop.placeId),
    );
    for (const candidate of this.candidates) {
      if (candidate.tripId !== p.tripId) continue;
      if (adoptedPlaceIds.has(candidate.placeId)) {
        if (candidate.status !== 'rejected') candidate.status = 'in_plan';
      } else if (candidate.status === 'in_plan') {
        candidate.status = 'shortlisted';
      }
    }
    this.recomputeFeasibility(newPlan.id);
    trip.currentPlanId = newPlan.id;
    p.status = 'applied';
    p.decidedBy = p.decidedBy ?? { kind: 'leader', userId: this.me };
    return newPlan;
  }

  private applyOp(op: ChangeOp, planId: string, tripId: string): void {
    if (op.op === 'remove_stop') {
      const stop = this.mustFindStopInPlan(planId, op.stopId);
      this.stops = this.stops.filter((item) => item !== stop);
    } else if (op.op === 'move_stop') {
      const stop = this.mustFindStopInPlan(planId, op.stopId);
      const previousDayId = stop.dayId;
      this.mustFindDayInPlan(planId, op.toDayId);
      stop.dayId = op.toDayId;
      stop.seq = op.seq;
      this.resequence(previousDayId);
      this.resequence(op.toDayId);
    } else if (op.op === 'add_stop') {
      this.mustFindDayInPlan(planId, op.dayId);
      this.mustFindPlaceForTrip(tripId, op.placeId);
      this.stops.push({
        id: this.id('s'),
        dayId: op.dayId,
        seq: op.seq,
        placeId: op.placeId,
        stopKind: op.stopKind,
        plannedArrival: '12:00',
        durationMin: 60,
        booking: null,
        notes: '',
      });
      this.resequence(op.dayId);
    } else if (op.op === 'add_place_stop') {
      this.mustFindDayInPlan(planId, op.dayId);
      // Materialise the drafted place first (Phase B geocodes it; the mock
      // drops it near the day's other stops so it lands on the map), then add
      // its stop. The draft's note seeds the stop's notes.
      const place = this.materialiseDraft(op.draft, op.dayId);
      this.places.push(place);
      this.stops.push({
        id: this.id('s'),
        dayId: op.dayId,
        seq: op.seq,
        placeId: place.id,
        stopKind: op.stopKind,
        plannedArrival: '12:00',
        durationMin: 60,
        booking: null,
        notes: op.draft.note,
      });
      this.resequence(op.dayId);
    } else if (op.op === 'reorder') {
      op.stopIdsInOrder.forEach((sid, i) => {
        const stop = this.mustFindStopInPlan(planId, sid);
        stop.seq = i + 1;
      });
    } else if (op.op === 'swap_place') {
      const stop = this.mustFindStopInPlan(planId, op.stopId);
      this.mustFindPlaceForTrip(tripId, op.newPlaceId);
      stop.placeId = op.newPlaceId;
    } else if (op.op === 'add_day') {
      this.days.push({
        id: this.id('d'),
        planId,
        date: op.date,
        cityHint: op.cityHint,
        tz: 'Asia/Tokyo',
        windowStart: '09:00',
        windowEnd: '21:00',
      });
    } else if (op.op === 'remove_day') {
      const day = this.mustFindDayInPlan(planId, op.dayId);
      const removedStops = new Set(this.stops.filter((stop) => stop.dayId === day.id));
      this.days = this.days.filter((item) => item !== day);
      this.stops = this.stops.filter((stop) => !removedStops.has(stop));
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
      guide: null,
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
      const notes =
        feasibility === 'ok' ? [] : [`${Math.round(pct * 100)}% of the day window used after the last change.`];
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

  async proposalToPoll(tripId: string, proposalId: string): Promise<Poll> {
    this.requireLeader(tripId);
    const p = this.mustFindForTrip(this.proposals, tripId, proposalId, 'proposal');
    if (p.decidedBy?.kind === 'poll') {
      const prior = this.mustFindForTrip(this.polls, tripId, p.decidedBy.pollId, 'poll');
      if (p.status !== 'pending' || !['passed', 'failed', 'expired'].includes(prior.status)) {
        return latency(clone(prior));
      }
      // A tied/below-quorum poll leaves the proposal pending, so a leader may
      // deliberately send that same proposal to a fresh vote.
    } else if (p.decidedBy) {
      throw new ApiError(409, 'proposal cannot be routed in its current state');
    }
    if (p.status !== 'pending') throw new ApiError(409, 'proposal cannot be routed in its current state');
    p.route = 'poll';
    const poll = this.openPlanChangePoll(p);
    return latency(clone(poll));
  }

  /**
   * Wrap a proposal in an immediately-open `plan_change` poll — the exact poll
   * the leader's "send it to a poll" path builds. Adopt-vs-keep options, quorum
   * at half the eligible voters, a 7-day window; the proposal's `decidedBy` points at
   * the poll and it stays `pending` until the poll closes (and applies on pass).
   */
  private openPlanChangePoll(p: Proposal): Poll {
    const poll: Poll = {
      id: this.id('poll'),
      tripId: p.tripId,
      createdBy: this.me,
      kind: 'plan_change',
      title: p.title,
      description: p.rationale,
      options: [
        { id: this.id('opt'), label: 'Adopt the proposed plan change', proposalId: p.id },
        { id: this.id('opt'), label: 'Keep the current plan', proposalId: null },
      ],
      opensAt: null,
      closesAt: daysFromNow(7),
      decidedAt: null,
      quorum: this.pollQuorum(p.tripId),
      allowMulti: false,
      status: 'open',
      resolutionNote: null,
      votes: [],
    };
    p.status = 'pending';
    p.decidedBy = { kind: 'poll', pollId: poll.id };
    this.polls.push(poll);
    return poll;
  }

  // --- Polls ------------------------------------------------------------------------

  async listPolls(tripId: string): Promise<Poll[]> {
    this.requireMember(tripId);
    return latency(clone(this.polls.filter((p) => p.tripId === tripId)));
  }

  async createPoll(tripId: string, input: CreatePollInput): Promise<Poll> {
    this.requireEditor(tripId);
    // `plan_change` polls are an internal projection of a proposal that has
    // already passed trip-scoped validation. Never let request JSON attach an
    // arbitrary proposal id to a public poll.
    if ((input as { kind?: string }).kind !== 'decision') {
      throw new ApiError(400, 'public poll creation only accepts decision polls');
    }
    if ((input.options as { proposalId?: unknown }[]).some((option) => Object.hasOwn(option, 'proposalId'))) {
      throw new ApiError(400, 'public poll options cannot reference proposals');
    }
    const title = input.title.trim();
    const description = input.description.trim();
    const labels = input.options.map((option) => option.label.trim());
    if (!title || title.length > 200 || description.length > 4000) {
      throw new ApiError(400, 'poll title or description is invalid');
    }
    if (
      labels.length < 2 ||
      labels.length > 6 ||
      labels.some((label) => !label || label.length > 200) ||
      new Set(labels).size !== labels.length
    ) {
      throw new ApiError(400, 'poll must have 2-6 unique options');
    }
    const closesAt = parseUtcInstant(input.closesAt);
    if (closesAt === null || closesAt <= Date.now()) {
      throw new ApiError(400, 'poll deadline must be in the future');
    }
    const poll: Poll = {
      id: this.id('poll'),
      tripId,
      createdBy: this.me,
      kind: input.kind,
      title,
      description,
      options: labels.map((label) => ({ id: this.id('opt'), label, proposalId: null })),
      opensAt: null,
      closesAt: input.closesAt,
      decidedAt: null,
      quorum: this.pollQuorum(tripId),
      allowMulti: input.allowMulti,
      status: 'open',
      resolutionNote: null,
      votes: [],
    };
    this.polls.push(poll);
    return latency(clone(poll));
  }

  async openPoll(tripId: string, pollId: string): Promise<Poll> {
    this.requireEditor(tripId);
    const poll = this.mustFindForTrip(this.polls, tripId, pollId, 'poll');
    // Only a leader or the poll's author may publish it, including retries.
    if (poll.createdBy !== this.me) this.requireLeader(poll.tripId);
    if (poll.status === 'open') return latency(clone(poll));
    if (poll.status !== 'draft' && poll.status !== 'scheduled')
      throw new ApiError(409, 'only a draft or scheduled poll can be opened');
    const closesAt = parseUtcInstant(poll.closesAt);
    if (closesAt === null) throw new ApiError(500, 'stored poll deadline is invalid');
    if (closesAt <= Date.now()) throw new ApiError(409, 'poll deadline has passed');
    poll.status = 'open';
    poll.opensAt = null;
    return latency(clone(poll));
  }

  async vote(tripId: string, pollId: string, optionIds: string[]): Promise<Poll> {
    this.requireEditor(tripId);
    const poll = this.mustFindForTrip(this.polls, tripId, pollId, 'poll');
    const unique = new Set(optionIds);
    const available = new Set(poll.options.map((option) => option.id));
    if (
      unique.size !== optionIds.length ||
      optionIds.length > poll.options.length ||
      (!poll.allowMulti && optionIds.length !== 1) ||
      optionIds.some((optionId) => !available.has(optionId))
    ) {
      throw new ApiError(400, 'invalid ballot');
    }
    const current = poll.votes
      .filter((vote) => vote.userId === this.me)
      .map((vote) => vote.optionId)
      .sort();
    const desired = [...optionIds].sort();
    if (current.length === desired.length && current.every((optionId, index) => optionId === desired[index])) {
      return latency(clone(poll));
    }
    const closesAt = parseUtcInstant(poll.closesAt);
    if (closesAt === null) throw new ApiError(500, 'stored poll deadline is invalid');
    if (poll.status !== 'open' || closesAt <= Date.now()) throw new ApiError(409, 'poll is not open for voting');
    poll.votes = poll.votes.filter((v) => v.userId !== this.me);
    for (const optionId of desired) poll.votes.push({ userId: this.me, optionId, at: now() });
    return latency(clone(poll));
  }

  async closePoll(tripId: string, pollId: string): Promise<Poll> {
    const poll = this.mustFindForTrip(this.polls, tripId, pollId, 'poll');
    this.requireLeader(tripId);
    if (poll.status === 'passed' || poll.status === 'failed' || poll.status === 'expired') {
      return latency(clone(poll));
    }
    if (poll.status !== 'open') throw new ApiError(409, 'only an open poll can be closed');
    // Stamp the moment the poll actually stopped taking votes. "Close now" ends
    // a poll *before* its scheduled `closesAt`, and reading `closesAt` back as
    // the decision date printed tomorrow's date on something decided today.
    poll.decidedAt = now();
    // Below quorum → expired (no decision). At/above → passed or failed by winner.
    // Quorum counts *voters*, not ballots: on an `allowMulti` poll one person
    // ticking three options is still one person, and `votes.length` would have
    // cleared quorum on their own.
    const voters = new Set(poll.votes.map((v) => v.userId)).size;
    if (voters < poll.quorum) {
      poll.status = 'expired';
      poll.resolutionNote = 'Closed below quorum — no decision recorded.';
      return latency(clone(poll));
    }
    const counts = new Map(poll.options.map((option) => [option.id, 0]));
    for (const v of poll.votes) {
      if (counts.has(v.optionId)) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
    }
    const topCount = Math.max(...counts.values());
    const topOptions = poll.options.filter((option) => counts.get(option.id) === topCount);
    // A tie is not an arbitrary win for whichever option happened to be first
    // in an array. It is a completed poll with no decision, and—critically for
    // plan_change polls—must never publish either structural proposal.
    if (topCount === 0 || topOptions.length !== 1) {
      poll.status = 'failed';
      poll.resolutionNote = 'Closed with a tied result — no decision recorded.';
      return latency(clone(poll));
    }
    if (topCount * 2 <= voters) {
      poll.status = 'failed';
      poll.resolutionNote = 'No option reached a majority - no decision recorded.';
      return latency(clone(poll));
    }
    const winningOption = topOptions[0];
    // A plan_change poll "passes" only when the adopt option (with a proposal) wins.
    if (poll.kind === 'plan_change') {
      if (winningOption?.proposalId) {
        const proposal = this.proposals.find(
          (item) => item.id === winningOption.proposalId && item.tripId === poll.tripId,
        );
        if (!proposal) {
          poll.status = 'failed';
          poll.resolutionNote = 'The winning proposal is no longer available — no plan change was applied.';
          return latency(clone(poll));
        }
        try {
          if (proposal.status !== 'applied') this.applyProposal(proposal);
          poll.status = 'passed';
        } catch (error) {
          if (!(error instanceof ApiError) || error.status !== 409) throw error;
          poll.status = 'failed';
          poll.resolutionNote = 'The winning proposal was based on an outdated plan — no plan change was applied.';
        }
      } else {
        const linkedProposalId = poll.options.find((option) => option.proposalId)?.proposalId;
        const proposal = linkedProposalId
          ? this.proposals.find((item) => item.id === linkedProposalId && item.tripId === poll.tripId)
          : undefined;
        if (!proposal || proposal.status !== 'pending' || proposal.decidedBy?.kind !== 'poll') {
          throw new ApiError(500, 'poll proposal link is corrupt');
        }
        proposal.status = 'rejected';
        proposal.rejectionReason = 'The group chose to keep the current plan.';
        poll.status = 'failed';
        poll.resolutionNote = 'The group chose to keep the current plan.';
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
    const item = this.mustFind(this.reviewItems, itemId, 'review item');
    if (item.kind === 'edit') {
      const edit = this.mustFind(this.edits, item.edit.id, 'edit');
      edit.status = 'applied';
      const pool: Record<string, { id: string }[]> = {
        stop: this.stops,
        day: this.days,
        notice: this.notices,
        candidate: this.candidates,
        trip: this.trips,
      };
      const target = pool[edit.entity]?.find((x) => x.id === edit.entityId);
      if (target) (target as Record<string, unknown>)[edit.field] = clone(edit.newValue);
    } else if (item.kind === 'proposal') {
      // Publishing, not applying — it still needs leader approval or a poll
      const p = this.mustFind(this.proposals, item.proposal.id, 'proposal');
      const trip = this.mustFind(this.trips, p.tripId, 'trip');
      const currentPlan = trip.currentPlanId ? this.mustFind(this.plans, trip.currentPlanId, 'plan') : null;
      if (!currentPlan || p.changeSet.basePlanVersion !== currentPlan.version) {
        p.status = 'stale';
        throw new ApiError(409, 'proposal is based on an outdated plan');
      }
      p.status = 'pending';
    } else if (item.kind === 'candidate') {
      if (!this.candidates.some((candidate) => candidate.id === item.candidate.id)) {
        this.candidates.push({ ...item.candidate, status: 'shortlisted' });
      }
    } else if (item.kind === 'comment') {
      if (!this.comments.some((comment) => comment.id === item.comment.id)) {
        const comment = clone(item.comment);
        this.comments.push(comment);
        const thread = this.mustFind(this.threads, comment.threadId, 'thread');
        thread.commentCount += 1;
        thread.lastActivityAt = comment.createdAt;
      }
    }
    this.takeReviewItem(itemId);
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
    this.requireMember(tripId);
    return latency(
      clone(
        this.threads
          .filter((thread) => thread.tripId === tripId)
          .sort(
            (left, right) => right.lastActivityAt.localeCompare(left.lastActivityAt) || right.id.localeCompare(left.id),
          ),
      ),
    );
  }

  async createThread(tripId: string, input: CreateThreadInput): Promise<Thread> {
    this.requireEditor(tripId);
    this.validateThreadAnchor(tripId, input.anchor);
    if (
      this.threads.some(
        (thread) => thread.tripId === tripId && JSON.stringify(thread.anchor) === JSON.stringify(input.anchor),
      )
    ) {
      throw new ApiError(409, 'a thread already exists on this anchor');
    }
    const title = requiredDiscussionText(input.title, 200, 'thread title');
    const body = requiredDiscussionText(input.body, 10_000, 'comment body');
    const createdAt = now();
    const thread: Thread = {
      id: this.id('th'),
      tripId,
      anchor: clone(input.anchor),
      title,
      commentCount: 1,
      lastActivityAt: createdAt,
    };
    // A thread is seeded by its first comment — the body of the composer.
    const comment: Comment = {
      id: this.id('cm'),
      threadId: thread.id,
      author: this.me,
      body,
      createdAt,
      reactions: [],
    };
    this.threads.push(thread);
    this.comments.push(comment);
    return latency(clone(thread));
  }

  async getComments(tripId: string, threadId: string): Promise<Comment[]> {
    this.requireMember(tripId);
    this.mustFindForTrip(this.threads, tripId, threadId, 'thread');
    return latency(
      clone(
        this.comments
          .filter((comment) => comment.threadId === threadId)
          .sort((left, right) => left.createdAt.localeCompare(right.createdAt) || left.id.localeCompare(right.id)),
      ),
    );
  }

  async addComment(tripId: string, threadId: string, body: string): Promise<Comment> {
    this.requireEditor(tripId);
    const thread = this.mustFindForTrip(this.threads, tripId, threadId, 'thread');
    if (thread.commentCount >= 1_000) throw new ApiError(409, 'this thread has reached its comment limit');
    const comment: Comment = {
      id: this.id('cm'),
      threadId,
      author: this.me,
      body: requiredDiscussionText(body, 10_000, 'comment body'),
      createdAt: now(),
      reactions: [],
    };
    this.comments.push(comment);
    thread.commentCount += 1;
    thread.lastActivityAt = comment.createdAt;
    return latency(clone(comment));
  }

  async setReaction(
    tripId: string,
    threadId: string,
    commentId: string,
    emoji: string,
    active: boolean,
  ): Promise<Comment> {
    this.requireEditor(tripId);
    this.mustFindForTrip(this.threads, tripId, threadId, 'thread');
    const comment = this.comments.find((item) => item.id === commentId && item.threadId === threadId);
    if (!comment) throw new ApiError(404, `comment ${commentId} not found in thread ${threadId}`);
    emoji = requiredDiscussionText(emoji, 16, 'reaction');
    if (
      Array.from(emoji).some((character) => {
        const codePoint = character.codePointAt(0) ?? 0;
        return /\s/u.test(character) || codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
      })
    ) {
      throw new ApiError(400, 'reaction must not contain whitespace or controls');
    }
    const existing = comment.reactions.find((r) => r.emoji === emoji);
    if (existing) {
      existing.userIds = existing.userIds.filter((userId) => userId !== this.me);
      if (active) existing.userIds.push(this.me);
      existing.userIds.sort();
      comment.reactions = comment.reactions.filter((r) => r.userIds.length > 0);
    } else if (active) {
      comment.reactions.push({ emoji, userIds: [this.me] });
    }
    comment.reactions.sort((left, right) => left.emoji.localeCompare(right.emoji));
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
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const expense: Expense = {
      id: '',
      tripId,
      paidBy: input.paidBy,
      amount: input.amount,
      currency: input.currency,
      fxRateToBase: fxRateBetween(input.currency, trip.baseCurrency),
      category: input.category,
      split: clone(input.split),
      note: input.note,
      receiptPhotoUrl: null,
      linkedStopId: input.linkedStopId ?? null,
      createdAt: now(),
    };
    this.validateExpense(trip, expense);
    this.validateExpenseStopLink(tripId, expense.id, expense.linkedStopId);
    expense.id = this.id('e');
    this.expenses.push(expense);
    this.syncExpenseStopLink(tripId, expense.id, expense.linkedStopId);
    return latency(clone(expense));
  }

  async updateExpense(tripId: string, expenseId: string, patch: ExpensePatch): Promise<Expense> {
    const expense = this.mustFindForTrip(this.expenses, tripId, expenseId, 'expense');
    const trip = this.mustFind(this.trips, tripId, 'trip');
    if (!Object.values(patch).some((value) => value !== undefined)) {
      throw new ApiError(400, 'expense patch must contain at least one field');
    }

    const next = clone(expense);
    if (patch.paidBy !== undefined) next.paidBy = patch.paidBy;
    if (patch.amount !== undefined) next.amount = patch.amount;
    if (patch.category !== undefined) next.category = patch.category;
    if (patch.split !== undefined) next.split = clone(patch.split);
    if (patch.note !== undefined) next.note = patch.note;
    if (patch.linkedStopId !== undefined) next.linkedStopId = patch.linkedStopId;
    // The frozen rate belongs to the *currency*, not to the row: correcting a
    // typo'd amount must not silently re-rate a month-old booking at today's
    // rate, but switching JPY → EUR makes the old rate meaningless.
    if (patch.currency !== undefined && patch.currency !== expense.currency) {
      next.currency = patch.currency;
      next.fxRateToBase = fxRateBetween(patch.currency, trip.baseCurrency);
    }
    // Check the complete merged row and its reverse stop link before either is
    // changed, preserving the PATCH endpoint's all-or-nothing contract.
    this.validateExpense(trip, next);
    if (next.linkedStopId !== expense.linkedStopId) {
      this.validateExpenseStopLink(tripId, expense.id, next.linkedStopId);
    }
    Object.assign(expense, next);
    this.syncExpenseStopLink(tripId, expense.id, expense.linkedStopId);
    return latency(clone(expense));
  }

  async deleteExpense(tripId: string, expenseId: string): Promise<void> {
    const i = this.expenses.findIndex((expense) => expense.id === expenseId && expense.tripId === tripId);
    if (i < 0) throw new ApiError(404, `expense ${expenseId} not found in trip ${tripId}`);
    const tripStops = this.currentStopsForTrip(tripId);
    this.expenses.splice(i, 1);
    for (const stop of tripStops) {
      if (stop.booking?.ledgerEntryId === expenseId) stop.booking = { ...stop.booking, ledgerEntryId: null };
    }
    return latency(undefined);
  }

  async addSettlement(tripId: string, input: AddSettlementInput): Promise<Settlement> {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const memberIds = new Set(trip.members.map((member) => member.userId));
    if (!memberIds.has(input.fromUser) || !memberIds.has(input.toUser)) {
      throw new ApiError(400, 'settlement participants must be current trip members');
    }
    if (input.fromUser === input.toUser) {
      throw new ApiError(400, 'a settlement must be between two different members');
    }
    if (!Number.isFinite(input.amount) || input.amount <= 0 || input.amount > 1_000_000_000) {
      throw new ApiError(400, 'settlement amount is invalid');
    }
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
      createdBy: this.me,
      category: input.category,
      title: input.title,
      body: input.body,
      sourceUrl: input.sourceUrl ?? null,
      pinned: false,
      status: 'active',
      audience: input.audience && input.audience.length ? input.audience : null,
      checklistItems: (input.checklistItems ?? []).map((text) => ({
        id: this.id('chk'),
        text,
        doneBy: [],
        mode: 'each',
      })),
    };
    this.notices.push(notice);
    return latency(clone(notice));
  }

  async toggleChecklistItem(tripId: string, noticeId: string, itemId: string): Promise<Notice> {
    const notice = this.mustFindForTrip(this.notices, tripId, noticeId, 'notice');
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

  private validateThreadAnchor(tripId: string, anchor: ThreadAnchor): void {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    if (anchor.kind === 'trip') return;
    if (anchor.kind === 'candidate') {
      this.mustFindForTrip(this.candidates, tripId, anchor.candidateId, 'candidate');
      return;
    }
    if (anchor.kind === 'poll') {
      this.mustFindForTrip(this.polls, tripId, anchor.pollId, 'poll');
      return;
    }
    const currentDayIds = new Set(this.days.filter((day) => day.planId === trip.currentPlanId).map((day) => day.id));
    if (anchor.kind === 'day') {
      if (!currentDayIds.has(anchor.dayId)) {
        throw new ApiError(404, `day ${anchor.dayId} not found in the current plan for trip ${tripId}`);
      }
      return;
    }
    const stop = this.stops.find((item) => item.id === anchor.stopId && currentDayIds.has(item.dayId));
    if (!stop) throw new ApiError(404, `stop ${anchor.stopId} not found in the current plan for trip ${tripId}`);
  }

  private id(prefix: string): string {
    return `${prefix}-${this.nextId++}`;
  }

  private mustFind<T extends { id: string }>(pool: T[], id: string, kind: string): T {
    const found = pool.find((x) => x.id === id);
    if (!found) throw new ApiError(404, `${kind} ${id} not found`);
    return found;
  }

  private mustFindForTrip<T extends { id: string; tripId: string }>(
    pool: T[],
    tripId: string,
    id: string,
    kind: string,
  ): T {
    const found = pool.find((item) => item.id === id && item.tripId === tripId);
    if (!found) throw new ApiError(404, `${kind} ${id} not found in trip ${tripId}`);
    return found;
  }

  /**
   * Return public catalog records plus reusable saved places belonging to this
   * trip. A materialised candidate-owned copy stays in the candidate picker
   * instead of appearing a second time, unless a plan has adopted it. Keeping
   * this ownership check next to the lookup mirrors the production rule: never
   * load a private place globally before authorizing its trip partition.
   */
  private searchablePlacesForTrip(tripId: string): Place[] {
    this.mustFind(this.trips, tripId, 'trip');
    const planIds = new Set(this.plans.filter((plan) => plan.tripId === tripId).map((plan) => plan.id));
    const dayIds = new Set(this.days.filter((day) => planIds.has(day.planId)).map((day) => day.id));
    const planPlaceIds = new Set(this.stops.filter((stop) => dayIds.has(stop.dayId)).map((stop) => stop.placeId));
    const candidatePlaceIds = this.candidates
      .filter((candidate) => candidate.tripId === tripId)
      .flatMap((candidate) => [candidate.placeId, ...(candidate.sourcePlaceId ? [candidate.sourcePlaceId] : [])]);
    const placeIds = new Set([...planPlaceIds, ...candidatePlaceIds]);
    const seen = new Set<string>();
    const saved = this.places.filter(
      (place) => placeIds.has(place.id) && (!this.candidateSnapshotIds.has(place.id) || planPlaceIds.has(place.id)),
    );
    return [...saved, ...this.catalog].filter((place) => {
      if (seen.has(place.id)) return false;
      seen.add(place.id);
      return true;
    });
  }

  private mustFindDayForTrip(tripId: string, dayId: string): Day {
    this.mustFind(this.trips, tripId, 'trip');
    const planIds = new Set(this.plans.filter((plan) => plan.tripId === tripId).map((plan) => plan.id));
    const day = this.days.find((item) => item.id === dayId && planIds.has(item.planId));
    if (!day) throw new ApiError(404, `day ${dayId} not found in trip ${tripId}`);
    return day;
  }

  private mustFindStopForTrip(tripId: string, stopId: string): Stop {
    const stop = this.stopsForTrip(tripId).find((item) => item.id === stopId);
    if (!stop) throw new ApiError(404, `stop ${stopId} not found in trip ${tripId}`);
    return stop;
  }

  private mustFindCurrentDayForTrip(tripId: string, dayId: string): Day {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const day = this.days.find((item) => item.id === dayId && item.planId === trip.currentPlanId);
    if (!day) throw new ApiError(409, `day ${dayId} is no longer part of the current plan`);
    return day;
  }

  private mustFindCurrentStopForTrip(tripId: string, stopId: string): Stop {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const currentDayIds = new Set(this.days.filter((day) => day.planId === trip.currentPlanId).map((day) => day.id));
    const stop = this.stops.find((item) => item.id === stopId && currentDayIds.has(item.dayId));
    if (!stop) throw new ApiError(409, `stop ${stopId} is no longer part of the current plan`);
    return stop;
  }

  private mustFindLedgerStopForTrip(tripId: string, stopId: string): Stop {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const currentDayIds = new Set(this.days.filter((day) => day.planId === trip.currentPlanId).map((day) => day.id));
    const stop = this.stops.find((item) => item.id === stopId && currentDayIds.has(item.dayId));
    if (!stop) throw new ApiError(404, `stop ${stopId} not found in trip ${tripId}`);
    return stop;
  }

  private stopsForTrip(tripId: string): Stop[] {
    this.mustFind(this.trips, tripId, 'trip');
    const planIds = new Set(this.plans.filter((plan) => plan.tripId === tripId).map((plan) => plan.id));
    const dayIds = new Set(this.days.filter((day) => planIds.has(day.planId)).map((day) => day.id));
    return this.stops.filter((stop) => dayIds.has(stop.dayId));
  }

  private currentStopsForTrip(tripId: string): Stop[] {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const dayIds = new Set(this.days.filter((day) => day.planId === trip.currentPlanId).map((day) => day.id));
    return this.stops.filter((stop) => dayIds.has(stop.dayId));
  }

  private validateExpense(trip: Trip, expense: Expense): void {
    if (!Number.isFinite(expense.amount) || expense.amount <= 0 || expense.amount > 1_000_000_000) {
      throw new ApiError(400, 'expense amount must be greater than zero');
    }
    if (!/^[A-Z]{3}$/.test(expense.currency)) {
      throw new ApiError(400, 'expense currency must be a three-letter ISO 4217 code');
    }
    if (!['lodging', 'food', 'transport', 'tickets', 'other'].includes(expense.category)) {
      throw new ApiError(400, `unsupported expense category ${expense.category}`);
    }

    const memberIds = new Set(trip.members.map((member) => member.userId));
    if (!memberIds.has(expense.paidBy)) throw new ApiError(400, 'expense payer must be a current trip member');

    let participants: { userId: string; value?: number }[];
    if (expense.split.kind === 'even') {
      participants = expense.split.participantIds.map((userId) => ({ userId }));
    } else if (expense.split.kind === 'shares') {
      participants = expense.split.participants.map(({ userId, weight }) => ({ userId, value: weight }));
      if (
        participants.some(({ value }) => !Number.isFinite(value) || (value ?? 0) <= 0 || (value ?? 0) > 1_000_000_000)
      ) {
        throw new ApiError(400, 'expense share weights must be greater than zero');
      }
    } else if (expense.split.kind === 'exact') {
      participants = expense.split.participants.map(({ userId, amount }) => ({ userId, value: amount }));
      if (
        participants.some(({ value }) => !Number.isFinite(value) || (value ?? -1) < 0 || (value ?? 0) > 1_000_000_000)
      ) {
        throw new ApiError(400, 'exact split amounts cannot be negative');
      }
      const exactTotal = participants.reduce((sum, participant) => sum + (participant.value ?? 0), 0);
      if (Math.abs(exactTotal - expense.amount) > 1e-6) {
        throw new ApiError(400, 'exact split amounts must equal the expense amount');
      }
    } else {
      throw new ApiError(400, 'unsupported expense split kind');
    }

    if (participants.length === 0 || participants.length > 50) {
      throw new ApiError(400, 'an expense needs between 1 and 50 participants');
    }
    const participantIds = participants.map(({ userId }) => userId);
    if (new Set(participantIds).size !== participantIds.length) {
      throw new ApiError(400, 'expense participants must be unique');
    }
    if (participantIds.some((userId) => !memberIds.has(userId))) {
      throw new ApiError(400, 'expense participants must be current trip members');
    }
    if (expense.note.length > 10_000) throw new ApiError(400, 'expense note is too long');
  }

  private validateExpenseStopLink(tripId: string, expenseId: string, linkedStopId: string | null): void {
    if (linkedStopId === null) return;
    const stop = this.mustFindLedgerStopForTrip(tripId, linkedStopId);
    if (!stop.booking) {
      throw new ApiError(409, `stop ${linkedStopId} has no booking to carry a ledger link`);
    }
    const existingExpenseId = stop.booking?.ledgerEntryId;
    if (existingExpenseId && existingExpenseId !== expenseId) {
      throw new ApiError(409, `stop ${linkedStopId} is already linked to another expense`);
    }
  }

  private syncExpenseStopLink(tripId: string, expenseId: string, linkedStopId: string | null): void {
    const tripStops = this.currentStopsForTrip(tripId);
    for (const stop of tripStops) {
      if (stop.id !== linkedStopId && stop.booking?.ledgerEntryId === expenseId) {
        stop.booking = { ...stop.booking, ledgerEntryId: null };
      }
    }
    const linkedStop = tripStops.find((stop) => stop.id === linkedStopId);
    if (linkedStop?.booking) linkedStop.booking = { ...linkedStop.booking, ledgerEntryId: expenseId };
  }

  private mustFindDayInPlan(planId: string, dayId: string): Day {
    const day = this.days.find((item) => item.id === dayId && item.planId === planId);
    if (!day) throw new ApiError(404, `day ${dayId} not found in current plan`);
    return day;
  }

  private mustFindStopInPlan(planId: string, stopId: string): Stop {
    const dayIds = new Set(this.days.filter((day) => day.planId === planId).map((day) => day.id));
    const stop = this.stops.find((item) => item.id === stopId && dayIds.has(item.dayId));
    if (!stop) throw new ApiError(404, `stop ${stopId} not found in current plan`);
    return stop;
  }

  /**
   * Resolve a place without ever treating its opaque id as proof of access.
   * Catalog places are public; saved/candidate places must be reachable from
   * this trip's own partition.
   */
  private mustFindPlaceForTrip(tripId: string, placeId: string): Place {
    this.mustFind(this.trips, tripId, 'trip');
    const catalogPlace = this.catalog.find((place) => place.id === placeId);
    if (catalogPlace) return catalogPlace;

    const planIds = new Set(this.plans.filter((plan) => plan.tripId === tripId).map((plan) => plan.id));
    const dayIds = new Set(this.days.filter((day) => planIds.has(day.planId)).map((day) => day.id));
    const permittedPlaceIds = new Set(this.stops.filter((stop) => dayIds.has(stop.dayId)).map((stop) => stop.placeId));
    for (const candidate of this.candidates.filter((item) => item.tripId === tripId)) {
      permittedPlaceIds.add(candidate.placeId);
      if (candidate.sourcePlaceId) permittedPlaceIds.add(candidate.sourcePlaceId);
    }

    const place = permittedPlaceIds.has(placeId) ? this.places.find((item) => item.id === placeId) : undefined;
    if (!place) throw new ApiError(404, `place ${placeId} not found in trip ${tripId}`);
    return place;
  }

  /**
   * Preflight a ChangeSet against one live plan. The small virtual day/stop map
   * makes validation order-aware, so combinations such as remove-then-move are
   * rejected before the real arrays are touched.
   */
  private validateChangeSet(tripId: string, plan: Plan, changeSet: ChangeSet): void {
    if (plan.tripId !== tripId) throw new ApiError(404, `plan ${plan.id} not found in trip ${tripId}`);
    if (changeSet.basePlanVersion !== plan.version) {
      throw new ApiError(
        409,
        `proposal is based on plan v${changeSet.basePlanVersion}; current plan is v${plan.version}`,
      );
    }
    if (!Array.isArray(changeSet.ops) || changeSet.ops.length === 0) {
      throw new ApiError(400, 'a structural proposal needs at least one operation');
    }

    const dayIds = new Set(this.days.filter((day) => day.planId === plan.id).map((day) => day.id));
    const currentStops = this.stops.filter((stop) => dayIds.has(stop.dayId));
    const stopDays = new Map(currentStops.map((stop) => [stop.id, stop.dayId]));
    const linkedStopIds = new Set(
      currentStops
        .filter((stop) => stop.booking?.ledgerEntryId !== null && stop.booking?.ledgerEntryId !== undefined)
        .map((stop) => stop.id),
    );
    const requireDay = (dayId: string): void => {
      if (!dayIds.has(dayId)) throw new ApiError(404, `day ${dayId} not found in current plan`);
    };
    const requireStop = (stopId: string): string => {
      const dayId = stopDays.get(stopId);
      if (!dayId) throw new ApiError(404, `stop ${stopId} not found in current plan`);
      return dayId;
    };
    const requireAdoptablePlace = (placeId: string): void => {
      this.mustFindPlaceForTrip(tripId, placeId);
      if (
        this.candidates.some(
          (candidate) =>
            candidate.tripId === tripId && candidate.placeId === placeId && candidate.status === 'rejected',
        )
      ) {
        throw new ApiError(400, 'a rejected candidate cannot be adopted');
      }
    };

    for (const op of changeSet.ops) {
      if (op.op === 'add_stop') {
        requireDay(op.dayId);
        requireAdoptablePlace(op.placeId);
      } else if (op.op === 'add_place_stop') {
        requireDay(op.dayId);
      } else if (op.op === 'remove_stop') {
        requireStop(op.stopId);
        if (linkedStopIds.has(op.stopId)) throw new ApiError(400, 'a linked stop must be unlinked before removal');
        stopDays.delete(op.stopId);
      } else if (op.op === 'move_stop') {
        requireStop(op.stopId);
        requireDay(op.toDayId);
        stopDays.set(op.stopId, op.toDayId);
      } else if (op.op === 'reorder') {
        requireDay(op.dayId);
        for (const stopId of op.stopIdsInOrder) {
          if (requireStop(stopId) !== op.dayId) {
            throw new ApiError(400, `stop ${stopId} does not belong to day ${op.dayId}`);
          }
        }
        const orderedIds = new Set(op.stopIdsInOrder);
        const dayStopIds = [...stopDays].filter(([, dayId]) => dayId === op.dayId).map(([stopId]) => stopId);
        if (
          orderedIds.size !== op.stopIdsInOrder.length ||
          orderedIds.size !== dayStopIds.length ||
          dayStopIds.some((stopId) => !orderedIds.has(stopId))
        ) {
          throw new ApiError(400, `reorder must contain every stop from day ${op.dayId} exactly once`);
        }
      } else if (op.op === 'swap_place') {
        requireStop(op.stopId);
        requireAdoptablePlace(op.newPlaceId);
      } else if (op.op === 'add_day') {
        // No nested ids: scalar/date validation belongs to request decoding.
      } else if (op.op === 'remove_day') {
        requireDay(op.dayId);
        if ([...stopDays].some(([stopId, dayId]) => dayId === op.dayId && linkedStopIds.has(stopId))) {
          throw new ApiError(400, 'a day containing a linked stop must be unlinked before removal');
        }
        dayIds.delete(op.dayId);
        for (const [stopId, dayId] of stopDays) {
          if (dayId === op.dayId) stopDays.delete(stopId);
        }
      } else {
        throw new ApiError(400, `unsupported structural operation ${(op as { op?: string }).op ?? ''}`);
      }
    }
  }

  private isLeader(tripId: string, userId: string): boolean {
    const trip = this.trips.find((t) => t.id === tripId);
    return trip?.members.some((m) => m.userId === userId && m.role === 'leader') ?? false;
  }

  private pollQuorum(tripId: string): number {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    return Math.ceil(trip.members.filter((member) => member.role !== 'viewer').length / 2);
  }

  private requireMember(tripId: string): void {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    if (!trip.members.some((member) => member.userId === this.me)) {
      throw new ApiError(404, `trip ${tripId} not found`);
    }
  }

  private requireEditor(tripId: string): void {
    const trip = this.mustFind(this.trips, tripId, 'trip');
    const role = trip.members.find((member) => member.userId === this.me)?.role;
    if (!role) throw new ApiError(404, `trip ${tripId} not found`);
    if (role === 'viewer') throw new ApiError(403, 'viewer role is read-only');
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
  private applyPatch<T extends { id: string }>(
    entity: Edit['entity'],
    target: T,
    patch: Partial<T>,
    tripId = 't-japan26',
  ): void {
    for (const [field, newValue] of Object.entries(patch)) {
      if (newValue === undefined) continue;
      const oldValue = (target as Record<string, unknown>)[field];
      if (JSON.stringify(oldValue) === JSON.stringify(newValue)) continue;
      this.recordEdit(entity, target.id, field, oldValue, newValue, tripId);
      (target as Record<string, unknown>)[field] = clone(newValue);
    }
  }

  private recordEdit(
    entity: Edit['entity'],
    entityId: string,
    field: string,
    oldValue: unknown,
    newValue: unknown,
    tripId: string,
  ): void {
    this.edits.push({
      id: this.id('ed'),
      tripId,
      entity,
      entityId,
      field,
      oldValue: clone(oldValue),
      newValue: clone(newValue),
      author: this.me,
      source: { via: 'web' },
      status: 'applied',
      createdAt: now(),
      revertedBy: null,
      revertedAt: null,
      revertEditId: null,
      revertsEditId: null,
    });
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

function requiredDiscussionText(value: string, maxChars: number, field: string): string {
  const normalised = value.trim();
  if (!normalised || Array.from(normalised).length > maxChars) {
    throw new ApiError(400, `${field} is required and must be at most ${maxChars.toLocaleString()} characters`);
  }
  return normalised;
}

function assertContentHistoryStorageBudget(edits: Edit[]): void {
  if (edits.length > CONTENT_HISTORY_SAFETY_LIMIT || encodedJsonBytes(edits, false) > CONTENT_HISTORY_BYTE_LIMIT) {
    throw new ApiError(409, 'this history operation exceeds the current safe processing limit');
  }
}

function assertContentHistoryResponseBudget(edits: Edit[]): void {
  if (encodedJsonBytes(edits, true) > CONTENT_HISTORY_BYTE_LIMIT) {
    throw new ApiError(409, 'this history operation exceeds the current safe processing limit');
  }
}

function visibleContentHistory(edits: Edit[]): ContentHistoryEdit[] {
  return edits.filter((edit): edit is ContentHistoryEdit => edit.status === 'applied' || edit.status === 'reverted');
}

function encodedJsonBytes(edits: Edit[], includeArrayEnvelope: boolean): number {
  let bytes = includeArrayEnvelope ? 2 : 0;
  for (const [index, edit] of edits.entries()) {
    bytes += new TextEncoder().encode(JSON.stringify(edit)).byteLength;
    if (includeArrayEnvelope && index > 0) bytes += 1;
    if (bytes > CONTENT_HISTORY_BYTE_LIMIT) return bytes;
  }
  return bytes;
}

// --- Ledger math (mirrors what the backend will implement) ---------------------

export function computeLedger(trip: Trip, expenses: Expense[], settlements: Settlement[]): LedgerView {
  const people = new Set(trip.members.map((member) => member.userId));
  for (const expense of expenses) {
    people.add(expense.paidBy);
    if (expense.split.kind === 'even') {
      expense.split.participantIds.forEach((userId) => people.add(userId));
    } else {
      expense.split.participants.forEach((participant) => people.add(participant.userId));
    }
  }
  for (const settlement of settlements) {
    people.add(settlement.fromUser);
    people.add(settlement.toUser);
  }
  const paid = new Map<string, number>();
  const owed = new Map<string, number>();
  for (const userId of people) {
    paid.set(userId, 0);
    owed.set(userId, 0);
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

  const balances = [...people].sort().map((userId) => {
    const p = round2(paid.get(userId) ?? 0);
    const o = round2(owed.get(userId) ?? 0);
    const net = round2(p - o + (settled.get(userId) ?? 0));
    return { userId, paid: p, owed: o, net };
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

/**
 * Greedy min-cash-flow: repeatedly match the largest debtor with the largest
 * creditor. Amounts are rounded to whole base units for real-world transfers;
 * the sub-unit rounding residual is absorbed onto the largest-magnitude balance
 * (so the set still nets to zero). Every transfer keeps its original debtor;
 * moving a small amount onto another debtor would make the suggestions no
 * longer reconcile to the displayed per-person balances.
 */
function minCashFlow(balances: { userId: string; net: number }[]): LedgerView['suggestedTransfers'] {
  const rounded = balances.map((b) => ({ userId: b.userId, net: roundHalfAwayFromZero(b.net) }));
  const residual = rounded.reduce((s, b) => s + b.net, 0);
  if (residual !== 0 && rounded.length) {
    let idx = 0;
    for (let i = 1; i < rounded.length; i++) if (Math.abs(rounded[i].net) > Math.abs(rounded[idx].net)) idx = i;
    rounded[idx].net -= residual;
  }

  const creditors = rounded.filter((b) => b.net > 0).map((b) => ({ ...b }));
  const debtors = rounded.filter((b) => b.net < 0).map((b) => ({ ...b }));
  creditors.sort((a, b) => b.net - a.net);
  debtors.sort((a, b) => a.net - b.net);
  const transfers: LedgerView['suggestedTransfers'] = [];
  let ci = 0;
  let di = 0;
  while (ci < creditors.length && di < debtors.length) {
    const amount = Math.min(creditors[ci].net, -debtors[di].net);
    if (amount > 0) transfers.push({ fromUser: debtors[di].userId, toUser: creditors[ci].userId, amount });
    creditors[ci].net -= amount;
    debtors[di].net += amount;
    if (creditors[ci].net === 0) ci++;
    if (debtors[di].net === 0) di++;
  }
  return transfers;
}

/**
 * Rate that multiplies an amount in `currency` into `base`.
 *
 * The table is to-USD, and the old helper returned it raw — correct only
 * because the one fixture trip happens to be USD-based. A EUR trip logging a
 * ¥10,000 dinner would have stored fxRateToBase = 0.0066 and reported €66
 * instead of €57. Divide through by the base's own rate.
 */
function fxRateBetween(currency: string, base: string): number {
  const rates: Record<string, number> = { JPY: 0.0066, EUR: 1.16, GBP: 1.34, USD: 1 };
  return (rates[currency] ?? 1) / (rates[base] ?? 1);
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

function parseUtcInstant(value: string): number | null {
  if (!/(?:Z|[+-]00:00)$/.test(value)) return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function freshenActivePollDeadlines(polls: Poll[]): Poll[] {
  let activeIndex = 0;
  return polls.map((poll) => {
    if (poll.status === 'passed' || poll.status === 'failed' || poll.status === 'expired') return poll;
    const closesAt = daysFromNow(3 + activeIndex * 2);
    const opensAt = poll.status === 'scheduled' ? daysFromNow(1 + activeIndex * 2) : null;
    activeIndex += 1;
    return { ...poll, opensAt, closesAt };
  });
}

/** Inclusive ISO-date range without local-time/DST drift. */
function datesInclusive(startDate: string, endDate: string): string[] {
  const start = Date.parse(`${startDate}T00:00:00Z`);
  const end = Date.parse(`${endDate}T00:00:00Z`);
  const dates: string[] = [];
  for (let value = start; value <= end; value += 86_400_000) dates.push(new Date(value).toISOString().slice(0, 10));
  return dates;
}

function round2(n: number): number {
  return roundHalfAwayFromZero(n * 100) / 100;
}

/** Keep JavaScript's negative-half behaviour aligned with Rust and the API contract. */
function roundHalfAwayFromZero(value: number): number {
  return Math.sign(value) * Math.round(Math.abs(value));
}

/** Minutes between two "HH:MM" local times on the same day. */
function minutesBetween(start: string, end: string): number {
  const [sh, sm] = start.split(':').map(Number);
  const [eh, em] = end.split(':').map(Number);
  return eh * 60 + em - (sh * 60 + sm);
}
