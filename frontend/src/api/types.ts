/**
 * Domain types — the vocabulary shared by every ApiClient implementation.
 * Mirrors docs/DESIGN.md §3. This file (with client.ts) IS the API contract:
 * when the frontend freezes, these types are exported as docs/openapi.yaml.
 *
 * Conventions:
 * - ids are opaque strings
 * - instants are ISO-8601 UTC strings (`2026-11-02T09:30:00Z`)
 * - local times are `HH:MM` strings interpreted in the day's timezone
 * - dates are `YYYY-MM-DD` strings
 */

// ---------------------------------------------------------------------------
// Users & membership

export interface User {
  id: string;
  email: string;
  displayName: string;
  avatarColor: string; // UI-assigned; stable per user
}

export type TripRole = 'leader' | 'member' | 'viewer';

export interface TripMember {
  userId: string;
  role: TripRole;
  joinedAt: string;
}

export interface Invite {
  id: string;
  tripId: string;
  email: string;
  invitedBy: string; // userId
  status: 'pending' | 'accepted';
  createdAt: string;
}

// ---------------------------------------------------------------------------
// Places & candidates

export type PlaceKind = 'sight' | 'food' | 'lodging' | 'activity' | 'transport_hub';

/**
 * Provider-independent context for exploring a place. Catalog entries may use
 * app-curated copy; candidate-owned place snapshots may use member-authored
 * copy. Itinerary-specific decisions and reminders remain on Candidate.pitch /
 * Candidate.tags and Stop.notes.
 */
export interface PlaceActivityIdea {
  title: string;
  /** Expanded context; omitted when the title is self-explanatory. */
  details?: string;
}

export interface PlaceGuide {
  /** Card-length orientation; normally one sentence / one to two lines. */
  summary: string;
  /** Longer context for the expanded place panel. */
  intro: string;
  /** Optional things to do at the place. Detail is progressive disclosure, not required copy. */
  activityIdeas: PlaceActivityIdea[];
  practicalTips: string[];
}

export interface Place {
  id: string;
  name: string;
  kind: PlaceKind;
  lat: number;
  lng: number;
  tz: string; // IANA timezone
  countryCode: string; // ISO 3166-1 alpha-2
  adminArea: string;
  city: string;
  address: string;
  /** Provider-agnostic pointer into the source catalog (PlaceCatalog port). */
  externalRef: { provider: string; placeId: string } | null;
  website: string | null;
  phone: string | null;
  rating: number | null; // 0–5
  priceLevel: number | null; // 1–4
  /** Cached provider JSON; UI treats it as display-only. */
  openingHours: { weekdayText: string[] } | null;
  photoUrls: string[]; // R2 keys in prod; any URL in fixtures
  /** Optional editorial guide; null for unenriched catalog/manual places. */
  guide: PlaceGuide | null;
}

export type CandidateStatus = 'shortlisted' | 'in_plan' | 'rejected';

/** Candidate states a member may choose directly; `in_plan` is proposal-owned. */
export type CandidateDisposition = 'shortlisted' | 'rejected';

export interface Candidate {
  id: string;
  tripId: string;
  /**
   * Catalog/plan place this idea was copied from. null means the member entered
   * the place manually. Candidate.placeId always points at the editable,
   * candidate-owned snapshot, so guide edits never rewrite this source.
   */
  sourcePlaceId: string | null;
  placeId: string;
  proposedBy: string; // userId
  createdAt: string;
  pitch: string;
  tags: string[];
  status: CandidateStatus;
}

/** Candidates are always served with their place joined in — no N+1 fetches. */
export interface CandidateWithPlace extends Candidate {
  place: Place;
}

// ---------------------------------------------------------------------------
// Trip → Plan → Day → Stop

export type TripStatus = 'dreaming' | 'planning' | 'booked' | 'ongoing' | 'done';

export interface Trip {
  id: string;
  name: string;
  coverPhotoUrl: string | null;
  /**
   * Per-trip theme accent (CSS color). Leader-settable; when null the UI
   * falls back to the app accent. App chrome always stays neutral — only
   * trip surfaces (cards, hero, tabs) pick this up via `--trip-accent`.
   */
  accentColor: string | null;
  /**
   * Leader-customisable display labels for stop kinds shown on plan cards
   * (e.g. rename "activity" to "onsen"). Missing keys fall back to the
   * built-in kind names.
   */
  stopKindLabels: Partial<Record<StopKind, string>> | null;
  status: TripStatus;
  startDate: string;
  endDate: string;
  baseCurrency: string; // ISO 4217
  /**
   * Optional per-person soft spending cap (leader-settable). Never blocks a
   * spend — the Ledger colours its budget bar toward amber as the group's
   * running total approaches `amount × members`. Absent/null hides the bar and
   * shows only the running total. May be entered in any currency; the bar
   * converts it to the trip base for the comparison.
   */
  softBudget?: { amount: number; currency: string } | null;
  members: TripMember[];
  currentPlanId: string | null;
  createdAt: string;
}

/** Card-sized projection for the trip list. */
export interface TripSummary {
  id: string;
  name: string;
  coverPhotoUrl: string | null;
  accentColor: string | null;
  status: TripStatus;
  startDate: string;
  endDate: string;
  memberCount: number;
  cities: string[]; // derived from the current plan, for the card subtitle
}

export interface Plan {
  id: string;
  tripId: string;
  version: number;
  createdFromProposalId: string | null;
  createdAt: string;
}

export interface Day {
  id: string;
  planId: string;
  date: string;
  cityHint: string;
  tz: string;
  windowStart: string; // "09:00" local — feasibility budget
  windowEnd: string; // "22:00"
}

export type StopKind = 'visit' | 'meal' | 'lodging' | 'activity' | 'transit';

export interface Booking {
  ref: string;
  url: string | null;
  cost: { amount: number; currency: string } | null;
  ledgerEntryId: string | null;
}

export interface Stop {
  id: string;
  dayId: string;
  seq: number; // order within the day
  placeId: string;
  stopKind: StopKind;
  plannedArrival: string; // "HH:MM" local
  durationMin: number;
  booking: Booking | null;
  notes: string;
}

export type TravelMode = 'walk' | 'transit' | 'drive' | 'flight';
export type Feasibility = 'ok' | 'tight' | 'unreasonable' | 'impossible';

/** Computed + cached by the RoutingEngine; never user-edited. */
export interface Leg {
  fromStopId: string;
  toStopId: string;
  mode: TravelMode;
  distanceM: number;
  durationMin: number;
  feasibility: Feasibility;
  /** Human-readable reason when feasibility is not 'ok'. */
  feasibilityNote: string | null;
  providerSnapshotAt: string;
}

/** Per-day verdict from the feasibility engine (§5). */
export interface DayFeasibility {
  dayId: string;
  feasibility: Feasibility;
  usedMin: number; // visits + legs
  windowMin: number;
  notes: string[];
}

/** Everything needed to render the map + timeline in one fetch. */
export interface PlanDetail {
  plan: Plan;
  days: Day[];
  stops: Stop[];
  legs: Leg[];
  dayFeasibility: DayFeasibility[];
  places: Place[]; // every place referenced by stops
}

// ---------------------------------------------------------------------------
// Change management (§3.3)

/**
 * A place proposed from scratch in the add-stop composer's "Somewhere new"
 * mode — a spot not yet in the catalog. On apply the backend geocodes it into
 * a full Place (Phase B); only these human-entered fields cross the wire, so
 * this is the shape the future `POST …/proposals` contract accepts inline.
 */
export interface NewPlaceDraft {
  name: string;
  kind: PlaceKind;
  city: string;
  note: string; // seeds the new stop's notes; '' when omitted
  url: string | null; // Google-Maps / website link; null when omitted
  /**
   * Coordinates when the drafter picked the spot off the map / search catalog;
   * null for a hand-typed place the backend must still geocode. On apply the
   * mock materialises the place here (falling back near the day's centroid when
   * null) so a materialised place never lands at 0,0.
   */
  lat: number | null;
  lng: number | null;
}

export type ChangeOp =
  | { op: 'add_stop'; dayId: string; placeId: string; seq: number; stopKind: StopKind }
  // Add a brand-new place *and* its stop in one op. Unlike `add_stop` (which
  // references an existing catalog place by id), the place doesn't exist yet:
  // `draft` is materialised into a Place on apply, then the stop is inserted.
  | { op: 'add_place_stop'; dayId: string; seq: number; stopKind: StopKind; draft: NewPlaceDraft }
  | { op: 'remove_stop'; stopId: string }
  | { op: 'move_stop'; stopId: string; toDayId: string; seq: number }
  | { op: 'reorder'; dayId: string; stopIdsInOrder: string[] }
  | { op: 'swap_place'; stopId: string; newPlaceId: string }
  | { op: 'add_day'; date: string; cityHint: string }
  | { op: 'remove_day'; dayId: string };

export interface ChangeSet {
  basePlanVersion: number;
  ops: ChangeOp[];
}

/** Where a change came from: the web UI, or an AI holding an API token. */
export type ChangeSource = { via: 'web' } | { via: 'token'; tokenId: string; tokenName: string };

export type ProposalRoute = 'leader_approval' | 'poll';
export type ProposalStatus = 'draft' | 'pending' | 'approved' | 'rejected' | 'applied' | 'stale';

export interface Proposal {
  id: string;
  tripId: string;
  createdBy: string; // userId (token owner when source is a token)
  source: ChangeSource;
  title: string;
  rationale: string;
  changeSet: ChangeSet;
  route: ProposalRoute;
  status: ProposalStatus;
  decidedBy: { kind: 'leader'; userId: string } | { kind: 'poll'; pollId: string } | null;
  /** Set when a leader rejects — the required "shown to <proposer>" message (§3.3). */
  rejectionReason: string | null;
  createdAt: string;
}

export type PollKind = 'decision' | 'plan_change';
/**
 * Lifecycle: `draft` (being written, no votes counted) → `scheduled` (auto-opens
 * at `opensAt`) → `open` → `passed` | `failed` | `expired`. A poll that closes
 * below quorum is `expired`; one that closes at/above quorum is `passed`/`failed`.
 * A tied top result is `failed`: no option wins and no structural proposal applies.
 */
export type PollStatus = 'draft' | 'scheduled' | 'open' | 'passed' | 'failed' | 'expired';

export interface PollOption {
  id: string;
  label: string;
  proposalId: string | null; // set for plan_change polls
}

export interface PollVote {
  userId: string;
  optionId: string;
  at: string;
}

export interface Poll {
  id: string;
  tripId: string;
  createdBy: string;
  kind: PollKind;
  title: string;
  description: string;
  options: PollOption[];
  /** When a `scheduled` poll auto-opens; null for polls that open immediately. */
  opensAt: string | null;
  closesAt: string;
  /**
   * When the poll actually stopped taking votes. Distinct from `closesAt`,
   * which is only the *scheduled* deadline: a leader closing a poll early ends
   * it before that, and the UI was stamping such polls with their future
   * deadline ("closed Sun 2 Aug" on 30 Jul). Null while the poll is still
   * open, and absent on records written before this field existed — readers
   * fall back to `min(closesAt, now)`.
   */
  decidedAt?: string | null;
  quorum: number;
  allowMulti: boolean;
  status: PollStatus;
  votes: PollVote[];
  /** How a below-quorum / off-poll decision was ultimately made (§3.3). */
  resolutionNote: string | null;
}

export type EditEntity = 'stop' | 'day' | 'candidate' | 'notice' | 'trip';
export type EditStatus = 'applied' | 'pending_review' | 'rejected' | 'reverted';

/** Field-level, revertible history for content edits. */
export interface Edit {
  id: string;
  tripId: string;
  entity: EditEntity;
  entityId: string;
  field: string;
  oldValue: unknown;
  newValue: unknown;
  author: string; // userId
  source: ChangeSource;
  status: EditStatus;
  createdAt: string;
}

/** An item awaiting the token owner's approval — the AI airlock (§7). */
export type ReviewItem =
  | { id: string; kind: 'edit'; edit: Edit }
  | { id: string; kind: 'proposal'; proposal: Proposal }
  | { id: string; kind: 'candidate'; candidate: Candidate; place: Place }
  | { id: string; kind: 'comment'; tripId: string; comment: Comment; threadTitle: string };

// ---------------------------------------------------------------------------
// Discussions (§3.4)

export type ThreadAnchor =
  | { kind: 'trip' }
  | { kind: 'day'; dayId: string }
  | { kind: 'stop'; stopId: string }
  | { kind: 'poll'; pollId: string }
  | { kind: 'candidate'; candidateId: string };

export interface Thread {
  id: string;
  tripId: string;
  anchor: ThreadAnchor;
  title: string;
  commentCount: number;
  lastActivityAt: string;
}

export interface Comment {
  id: string;
  threadId: string;
  author: string; // userId
  body: string; // markdown
  createdAt: string;
  reactions: { emoji: string; userIds: string[] }[];
}

// ---------------------------------------------------------------------------
// Ledger (§3.5)

export type ExpenseCategory = 'lodging' | 'food' | 'transport' | 'tickets' | 'other';

export type ExpenseSplit =
  | { kind: 'even'; participantIds: string[] }
  | { kind: 'shares'; participants: { userId: string; weight: number }[] }
  | { kind: 'exact'; participants: { userId: string; amount: number }[] };

export interface Expense {
  id: string;
  tripId: string;
  paidBy: string; // userId
  amount: number; // in `currency`
  currency: string;
  fxRateToBase: number; // frozen at entry time
  category: ExpenseCategory;
  split: ExpenseSplit;
  note: string;
  receiptPhotoUrl: string | null;
  linkedStopId: string | null;
  createdAt: string;
}

export interface Settlement {
  id: string;
  tripId: string;
  fromUser: string;
  toUser: string;
  amount: number; // in trip base currency
  settledAt: string;
}

/** Computed server-side; never stored. All amounts in trip base currency. */
export interface LedgerView {
  expenses: Expense[];
  settlements: Settlement[];
  balances: { userId: string; paid: number; owed: number; net: number }[];
  /** Min-cash-flow simplification: who should pay whom to zero out. */
  suggestedTransfers: { fromUser: string; toUser: string; amount: number }[];
}

// ---------------------------------------------------------------------------
// Notices (§3.6)

export type NoticeCategory = 'visa' | 'safety' | 'health' | 'money' | 'connectivity' | 'packing' | 'custom';

export interface ChecklistItem {
  id: string;
  text: string;
  doneBy: string[]; // userIds — per-person checkable
  /**
   * Structured deadline (YYYY-MM-DD) the "what's still open" roll-up sorts and
   * badges by. Optional — items without a due date just never badge as "soon".
   * Replaces dates previously baked into `text` ("form due Nov 1").
   */
  dueDate?: string;
  /**
   * How the item is completed:
   * - `each` (default): every member ticks it individually — coverage is "n / 6".
   * - `group`: one shared task ("reserve the seats for everyone") — a single
   *   checkbox, stamped with whoever did it; done once `doneBy.length ≥ 1`.
   */
  mode?: 'each' | 'group';
}

export interface Notice {
  id: string;
  tripId: string;
  /** Member who posted the notice. Authors and trip leaders may manage it. */
  createdBy: string; // userId
  category: NoticeCategory;
  title: string;
  body: string; // markdown
  sourceUrl: string | null;
  pinned: boolean;
  /**
   * Lifecycle (default `active` when absent). `resolved` notices grey out and
   * sink below active ones; `archived` ones leave the visible list (kept in
   * history). Set via `NoticePatch.status`.
   */
  status?: 'active' | 'resolved' | 'archived';
  /**
   * Who the checklist obligations apply to (userIds). Absent/null = the whole
   * group. Members outside the audience still see the notice, but it isn't on
   * their personal list — checklist denominators and `personalOpenCount` count
   * only the audience.
   */
  audience?: string[] | null;
  checklistItems: ChecklistItem[];
}

// ---------------------------------------------------------------------------
// AI API tokens (§7)

export type TokenScope = 'read' | 'propose';

export interface ApiToken {
  id: string;
  name: string;
  prefix: string; // first chars of the plaintext, for recognition
  scopes: TokenScope[];
  expiresAt: string;
  lastUsedAt: string | null;
  revokedAt: string | null;
  createdAt: string;
}

/** Returned once, at creation — the plaintext is never retrievable again. */
export interface CreatedToken {
  token: ApiToken;
  plaintext: string; // itn_…
}
