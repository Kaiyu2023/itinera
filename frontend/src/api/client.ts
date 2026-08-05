/**
 * ApiClient — the port between the UI and any backend (DESIGN.md §2.1, §12).
 *
 * The frontend NEVER calls fetch directly; every data access goes through this
 * interface. Phase A implements it with MockApiClient (in-memory fixtures);
 * Phase B swaps in HttpApiClient against the Rust backend. Freezing the
 * frontend = freezing this interface (exported as docs/openapi.yaml).
 *
 * Trip-owned child methods always carry tripId as well as the child id. That
 * lets the backend authorize against the trip partition before addressing the
 * child; an opaque child id is never a global lookup key or permission proof.
 *
 * Method ↔ route mapping follows DESIGN.md §8.
 */

import type {
  ApiToken,
  CandidateDisposition,
  CandidateWithPlace,
  ChangeSet,
  Comment,
  CreatedToken,
  Day,
  Edit,
  Expense,
  ExpenseCategory,
  ExpenseSplit,
  Invite,
  LedgerView,
  Notice,
  NoticeCategory,
  Place,
  PlaceGuide,
  PlaceKind,
  Plan,
  PlanDetail,
  Poll,
  PollKind,
  Proposal,
  ProposalRoute,
  ReviewItem,
  Settlement,
  Stop,
  Thread,
  ThreadAnchor,
  TokenScope,
  Trip,
  TripStatus,
  TripSummary,
  User,
} from './types';

// --- Input shapes ----------------------------------------------------------

export interface CreateTripInput {
  name: string;
  startDate: string;
  endDate: string;
  baseCurrency: string;
}

/**
 * Bootstrap the first dated plan from a trip idea.
 *
 * A plan needs a timezone and a useful city label before its day rows can be
 * drawn. The first idea supplies both through its candidate-owned place
 * snapshot; subsequent structural changes continue through proposals.
 */
export interface InitializePlanInput {
  anchorPlaceId: string;
}

/**
 * Human-authored place fields available in the trip-idea composer.
 *
 * Provider-owned facts (coordinates, timezone, country, rating, price and
 * externalRef) intentionally stay out of this shape. When an existing catalog
 * place is used as a source the backend inherits those facts; when the member
 * changes any field below it creates candidate-scoped copy instead of mutating
 * the shared catalog or a place already used by the itinerary.
 */
export interface CandidatePlaceInput {
  name: string;
  kind: PlaceKind;
  city: string;
  address: string;
  website: string | null;
  phone: string | null;
  openingHours: string[];
  photoUrls: string[];
  /** null means this idea has no authored guide content yet. */
  guide: PlaceGuide | null;
}

export interface AddCandidateInput {
  /** Public catalog or same-trip place to inherit provider facts from; null for manual entry. */
  sourcePlaceId: string | null;
  place: CandidatePlaceInput;
  pitch: string;
  tags: string[];
}

export interface UpdateCandidateInput {
  place: CandidatePlaceInput;
  pitch: string;
  tags: string[];
}

/** Content-editable fields (§3.3) — everything else is structural. */
export interface StopPatch {
  plannedArrival?: string;
  durationMin?: number;
  notes?: string;
  booking?: Stop['booking'];
}

export interface DayPatch {
  windowStart?: string;
  windowEnd?: string;
  cityHint?: string;
}

export interface NoticePatch {
  title?: string;
  body?: string;
  pinned?: boolean;
  sourceUrl?: string | null;
  status?: 'active' | 'resolved' | 'archived';
  audience?: string[] | null;
}

export interface CreateProposalInput {
  title: string;
  rationale: string;
  changeSet: ChangeSet;
  route: ProposalRoute;
}

export interface CreatePollInput {
  kind: PollKind;
  title: string;
  description: string;
  options: { label: string; proposalId?: string }[];
  closesAt: string;
  allowMulti: boolean;
}

export interface AddExpenseInput {
  paidBy: string;
  amount: number;
  currency: string;
  category: ExpenseCategory;
  split: ExpenseSplit;
  note: string;
  linkedStopId?: string;
}

/**
 * Every field the add-expense composer can set, all optional.
 *
 * Expenses are *records*, not plan edits: they apply immediately and face no
 * approval, which is exactly why they must be correctable. Without this the UI
 * was write-only — a fat-fingered ¥140,000 for a ¥14,000 dinner skewed every
 * balance and every suggested transfer permanently, and the only recourse was
 * a compensating fake expense. Not routed through the edit/history machinery
 * (§3.3): that governs *plan* fields, and a ledger row is not part of the plan.
 */
export interface ExpensePatch {
  paidBy?: string;
  amount?: number;
  currency?: string;
  category?: ExpenseCategory;
  split?: ExpenseSplit;
  note?: string;
  /** null clears the link; undefined leaves it alone. */
  linkedStopId?: string | null;
}

export interface AddSettlementInput {
  fromUser: string;
  toUser: string;
  amount: number;
}

export interface CreateNoticeInput {
  category: NoticeCategory;
  title: string;
  body: string;
  sourceUrl?: string;
  checklistItems?: string[];
  /** userIds the notice's checklist obligations apply to; omit/undefined = whole group. */
  audience?: string[];
}

/** Seeds a new discussion thread with its first comment (the thread body). */
export interface CreateThreadInput {
  anchor: ThreadAnchor;
  title: string;
  body: string;
}

export interface CreateTokenInput {
  name: string;
  scopes: TokenScope[];
  ttlHours: 1 | 8 | 24 | 168;
}

// --- The port ---------------------------------------------------------------

export interface ApiClient {
  // Identity
  getMe(): Promise<User>;

  // Trips & membership
  listTrips(): Promise<TripSummary[]>;
  getTrip(tripId: string): Promise<Trip>;
  createTrip(input: CreateTripInput): Promise<Trip>;
  /**
   * Move a trip along its lifecycle: dreaming → planning → booked → ongoing →
   * done. Not governance-gated — the phase a trip is in is a fact about it, not
   * a change to the plan, so it applies immediately like a candidate's status.
   *
   * Backwards is a legal move and deliberately unguarded. Bookings fall
   * through, dates slip, and a trip that has to go from `booked` back to
   * `planning` is exactly the moment you least want the app arguing with you.
   */
  setTripStatus(tripId: string, status: TripStatus): Promise<Trip>;
  getUsers(tripId: string): Promise<User[]>; // members' profiles, for name/avatar display
  invite(tripId: string, email: string): Promise<Invite>;
  removeMember(tripId: string, userId: string): Promise<void>;

  // Places & candidates
  /** Search the public catalog plus reusable saved places owned by this trip. */
  searchPlaces(tripId: string, query: string): Promise<Place[]>; // PlaceCatalog port, server-side
  listCandidates(tripId: string): Promise<CandidateWithPlace[]>;
  addCandidate(tripId: string, input: AddCandidateInput): Promise<CandidateWithPlace>;
  /** Edit the idea and its candidate-scoped place copy immediately. */
  updateCandidate(tripId: string, candidateId: string, input: UpdateCandidateInput): Promise<CandidateWithPlace>;
  /**
   * Move a candidate between shortlist states. Candidates aren't governance-
   * gated (§3.2) so this applies immediately, like `addCandidate`.
   *
   * Without it `rejected` was a state the type system declared and no code path
   * could ever produce: the Candidates tab could only ever *read* a rejected
   * fixture. `in_plan` stays server-owned — a candidate becomes part of the
   * plan by a proposal being applied, never by someone tapping a chip — so the
   * UI only ever asks for `shortlisted` or `rejected` here.
   */
  setCandidateStatus(tripId: string, candidateId: string, status: CandidateDisposition): Promise<CandidateWithPlace>;

  // Plan
  getCurrentPlan(tripId: string): Promise<PlanDetail>;
  /** Idempotently create Plan v1 and one empty Day for every trip date. */
  initializePlan(tripId: string, input: InitializePlanInput): Promise<PlanDetail>;
  listPlanVersions(tripId: string): Promise<Plan[]>;

  // Content edits (immediate, history-logged)
  updateStop(tripId: string, stopId: string, patch: StopPatch): Promise<Stop>;
  updateDay(tripId: string, dayId: string, patch: DayPatch): Promise<Day>;
  updateNotice(tripId: string, noticeId: string, patch: NoticePatch): Promise<Notice>;
  getHistory(tripId: string): Promise<Edit[]>;
  revertEdit(tripId: string, editId: string): Promise<void>;

  // Structural proposals
  listProposals(tripId: string): Promise<Proposal[]>;
  createProposal(tripId: string, input: CreateProposalInput): Promise<Proposal>;
  approveProposal(tripId: string, proposalId: string): Promise<Proposal>; // leader only — applies + bumps the plan version
  rejectProposal(tripId: string, proposalId: string, reason: string): Promise<Proposal>; // leader only
  proposalToPoll(tripId: string, proposalId: string): Promise<Poll>; // leader declines to decide

  // Polls
  listPolls(tripId: string): Promise<Poll[]>;
  createPoll(tripId: string, input: CreatePollInput): Promise<Poll>;
  openPoll(tripId: string, pollId: string): Promise<Poll>; // publish a draft, or open a scheduled poll now
  vote(tripId: string, pollId: string, optionIds: string[]): Promise<Poll>; // cast or change your vote while open
  /** Leader only; ties fail, and plan_change polls apply only against their exact base plan version. */
  closePoll(tripId: string, pollId: string): Promise<Poll>;

  // AI airlock — the caller's own review queue
  getReviewQueue(): Promise<ReviewItem[]>;
  approveReviewItem(itemId: string): Promise<void>;
  rejectReviewItem(itemId: string): Promise<void>;

  // Discussions
  listThreads(tripId: string): Promise<Thread[]>;
  createThread(tripId: string, input: CreateThreadInput): Promise<Thread>;
  getComments(tripId: string, threadId: string): Promise<Comment[]>;
  addComment(tripId: string, threadId: string, body: string): Promise<Comment>;
  toggleReaction(tripId: string, threadId: string, commentId: string, emoji: string): Promise<Comment>;

  // Ledger
  getLedger(tripId: string): Promise<LedgerView>;
  addExpense(tripId: string, input: AddExpenseInput): Promise<Expense>;
  /** Correct a record after the fact. Re-freezes `fxRateToBase` iff `currency` changes. */
  updateExpense(tripId: string, expenseId: string, patch: ExpensePatch): Promise<Expense>;
  /** Remove a record entirely — the only honest fix for one that never happened. */
  deleteExpense(tripId: string, expenseId: string): Promise<void>;
  addSettlement(tripId: string, input: AddSettlementInput): Promise<Settlement>;

  // Notices
  listNotices(tripId: string): Promise<Notice[]>;
  createNotice(tripId: string, input: CreateNoticeInput): Promise<Notice>;
  toggleChecklistItem(tripId: string, noticeId: string, itemId: string): Promise<Notice>;

  // AI API tokens
  listTokens(): Promise<ApiToken[]>;
  createToken(input: CreateTokenInput): Promise<CreatedToken>;
  revokeToken(tokenId: string): Promise<void>;
}
