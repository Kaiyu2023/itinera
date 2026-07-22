/**
 * ApiClient — the port between the UI and any backend (DESIGN.md §2.1, §12).
 *
 * The frontend NEVER calls fetch directly; every data access goes through this
 * interface. Phase A implements it with MockApiClient (in-memory fixtures);
 * Phase B swaps in HttpApiClient against the Rust backend. Freezing the
 * frontend = freezing this interface (exported as docs/openapi.yaml).
 *
 * Method ↔ route mapping follows DESIGN.md §8.
 */

import type {
  ApiToken,
  CandidateWithPlace,
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
  PlanDetail,
  Plan,
  Poll,
  PollKind,
  Proposal,
  ProposalRoute,
  ChangeSet,
  ReviewItem,
  Settlement,
  Stop,
  Thread,
  TokenScope,
  Trip,
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

export interface AddCandidateInput {
  placeId: string;
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
  getUsers(tripId: string): Promise<User[]>; // members' profiles, for name/avatar display
  invite(tripId: string, email: string): Promise<Invite>;
  removeMember(tripId: string, userId: string): Promise<void>;

  // Places & candidates
  searchPlaces(query: string): Promise<Place[]>; // PlaceCatalog port, server-side
  listCandidates(tripId: string): Promise<CandidateWithPlace[]>;
  addCandidate(tripId: string, input: AddCandidateInput): Promise<CandidateWithPlace>;

  // Plan
  getCurrentPlan(tripId: string): Promise<PlanDetail>;
  listPlanVersions(tripId: string): Promise<Plan[]>;

  // Content edits (immediate, history-logged)
  updateStop(stopId: string, patch: StopPatch): Promise<Stop>;
  updateDay(dayId: string, patch: DayPatch): Promise<Day>;
  updateNotice(noticeId: string, patch: NoticePatch): Promise<Notice>;
  getHistory(tripId: string): Promise<Edit[]>;
  revertEdit(editId: string): Promise<void>;

  // Structural proposals
  listProposals(tripId: string): Promise<Proposal[]>;
  createProposal(tripId: string, input: CreateProposalInput): Promise<Proposal>;
  approveProposal(proposalId: string): Promise<Proposal>; // leader only — applies + bumps the plan version
  rejectProposal(proposalId: string, reason: string): Promise<Proposal>; // leader only; reason shown to proposer
  proposalToPoll(proposalId: string): Promise<Poll>; // leader declines to decide

  // Polls
  listPolls(tripId: string): Promise<Poll[]>;
  createPoll(tripId: string, input: CreatePollInput): Promise<Poll>;
  openPoll(pollId: string): Promise<Poll>; // publish a draft, or open a scheduled poll now
  vote(pollId: string, optionIds: string[]): Promise<Poll>; // cast or change your vote while open
  closePoll(pollId: string): Promise<Poll>; // leader only; plan_change polls apply on pass

  // AI airlock — the caller's own review queue
  getReviewQueue(): Promise<ReviewItem[]>;
  approveReviewItem(itemId: string): Promise<void>;
  rejectReviewItem(itemId: string): Promise<void>;

  // Discussions
  listThreads(tripId: string): Promise<Thread[]>;
  getComments(threadId: string): Promise<Comment[]>;
  addComment(threadId: string, body: string): Promise<Comment>;
  toggleReaction(commentId: string, emoji: string): Promise<Comment>; // add/remove your reaction

  // Ledger
  getLedger(tripId: string): Promise<LedgerView>;
  addExpense(tripId: string, input: AddExpenseInput): Promise<Expense>;
  addSettlement(tripId: string, input: AddSettlementInput): Promise<Settlement>;

  // Notices
  listNotices(tripId: string): Promise<Notice[]>;
  createNotice(tripId: string, input: CreateNoticeInput): Promise<Notice>;
  toggleChecklistItem(noticeId: string, itemId: string): Promise<Notice>;

  // AI API tokens
  listTokens(): Promise<ApiToken[]>;
  createToken(input: CreateTokenInput): Promise<CreatedToken>;
  revokeToken(tokenId: string): Promise<void>;
}
