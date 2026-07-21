# Itinera — Design Document

Status: draft v1 · 2026-07-21 · author: Kaiyu Huang + Claude

Itinera is a collaborative trip planner for a small group of friends. A trip is a
multi-day route drawn on a map; the group proposes candidate places, votes on
changes through polls, discusses in threads, splits costs in a shared ledger, and
can let AI assistants participate via short-lived scoped API tokens.

---

## 1. Guiding principles

1. **Everything behind an interface.** Every external dependency — maps, routing,
   email, database, object storage — is accessed through a Rust trait (backend) or
   TypeScript interface (frontend). Callers never import a vendor SDK directly.
   Swapping Google Maps for MapLibre/OSRM, or Neon for DynamoDB, must not touch
   business logic.
2. **Two-tier governance, everything historied.** *Structural* changes (anything
   that reshapes the route: add/remove/move stops or days) require a poll **or**
   a leader's approval. *Content* edits (text, times, photos, bookings) apply
   immediately but are recorded in a field-level, revertible edit history.
   AI-originated changes of either kind are staged for the token owner's
   personal review before they enter the system at all.
3. **As close to $0/month as possible.** Every component is chosen to fit a
   permanent free tier at friend-group scale. See §10 for the cost table.
4. **Phone-first viewing, laptop-first editing.** The people on the trip will
   mostly *read* the plan on a phone; heavy editing happens beforehand on a laptop.

---

## 2. Architecture overview

```
 ┌────────────────────────────┐
 │  Frontend (React + TS)     │  Cloudflare Pages (free, CDN, custom domain)
 │  MapView / DayView / Polls │
 └────────────┬───────────────┘
              │ HTTPS (JSON API + cookie session or Bearer token)
 ┌────────────▼───────────────┐
 │  Cloudflare (free tier)    │  DNS, TLS, caching, WAF,
 │  Access one-time PIN login │  Zero Trust free plan (≤ 50 users)
 └────────────┬───────────────┘
              │
 ┌────────────▼───────────────┐
 │  AWS Lambda (Rust, axum)   │  single "monolith" function via Function URL
 │  ┌───────────────────────┐ │  (Function URLs are free — no API Gateway cost)
 │  │ domain core (traits)  │ │
 │  └──┬──────┬──────┬──────┘ │
 └─────┼──────┼──────┼────────┘
       │      │      │ adapter crates (one per provider)
   Postgres  Google  Cloudflare R2
   (Neon)    Maps    (photos, free 10 GB)
```

- **One Lambda, not microservices.** A single axum app compiled with
  `cargo-lambda` + `lambda_http`. At this scale, splitting functions only adds
  cold starts and deployment complexity.
- **Lambda Function URL instead of API Gateway** (saves API Gateway pricing
  entirely). Cloudflare sits in front for the custom domain, TLS, caching and
  Turnstile. The Function URL hostname is kept secret + verified via a shared
  header so traffic must come through Cloudflare.
- **Database: Postgres on Neon free tier.** The domain (polls, ledger splits,
  threaded comments, plan diffs) is relational; SQL keeps invariants simple.
  Accessed only through repository traits, so a later move to DynamoDB or
  SQLite/Turso is an adapter swap. Neon autosuspends when idle — fits the
  bursty usage of a friend group.

### 2.1 Ports (traits) — the swappability contract

```rust
// backend/crates/core/src/ports/
trait PlaceCatalog   { fn search(&self, q) -> …; fn details(&self, ref) -> …; fn photo_url(&self, ref) -> …; }
trait RoutingEngine  { fn leg(&self, from, to, mode, depart) -> Leg; fn matrix(&self, pts, mode) -> …; }
trait IdentityProvider { fn authenticate(&self, req) -> Identity; }  // Cloudflare Access JWT adapter;
                                                                     // fallback adapter: self-hosted OTP + SES
trait Mailer         { fn send_digest(&self, …) -> …; }              // v2 only (digests/invites) — login needs no email from us
trait BlobStore      { fn put(&self, key, bytes) -> Url; }
trait Clock / IdGen  // deterministic tests

// repositories
trait TripRepo, PlanRepo, PollRepo, LedgerRepo, UserRepo, TokenRepo, CommentRepo
```

Adapters: `adapter-gmaps` (implements `PlaceCatalog` + `RoutingEngine` with
Places API + Routes API), `adapter-cf-access`, `adapter-postgres`, `adapter-r2`.
The frontend mirrors this with a `MapRenderer` interface implemented by
`GoogleMapRenderer` (and later, potentially, `MapLibreRenderer`).

### 2.2 Repository layout (monorepo)

```
itinera/
├── backend/                 # Rust workspace
│   ├── crates/core/         # domain types, ports, services (no vendor deps)
│   ├── crates/adapters/     # gmaps, ses, postgres, r2
│   └── crates/api/          # axum routes, auth middleware, lambda entrypoint
├── frontend/                # Vite + React + TypeScript
├── infra/                   # IaC (SAM or Terraform), deploy scripts
└── docs/
```

---

## 3. Data model

Hierarchy: **Trip → Plan (versioned) → Day → Stop**, with a shared **Place**
catalog underneath and **Candidates** linking places to trips.

### 3.1 Places & candidates

```
Place                          # global catalog entry, provider-agnostic
  id, name, kind               # kind: sight | food | lodging | activity | transport_hub
  lat, lng, tz                 # IANA timezone, resolved once at import
  country_code, admin_area, city, address
  external_ref                 # e.g. {provider: "google", place_id: "…"} — behind PlaceCatalog
  website, phone, rating, price_level, opening_hours (cached JSON)
  photo_keys[]                 # R2 keys; photos cached to our storage (see §9 ToS note)

Candidate                      # "shortlist" of a trip — the pool polls choose from
  id, trip_id, place_id
  proposed_by, created_at
  pitch                        # why this place — free text
  tags[]                       # "must-see", "rainy-day", "splurge"…
  status                       # shortlisted | in_plan | rejected
```

Country → city → place emerges from `country_code / city` on Place; there is no
separate Country/City table to maintain — the hierarchy is derived for grouping
in the UI. Candidates can therefore trivially span cities and countries.

### 3.2 Trip, plan, days, stops

```
Trip
  id, name, cover_photo, status        # dreaming | planning | booked | ongoing | done
  start_date, end_date                 # dates only; times are per-day, local
  base_currency                        # for the ledger
  members[] {user_id, role}            # leader | member | viewer
                                       # ≥1 leader required; the creator starts as leader;
                                       # leaders approve structural changes & manage settings
  notices[]                            # see §3.6

Plan                                   # a full itinerary; ONLY created by applied proposals
  id, trip_id, version, created_from_poll_id, created_at
  # Trip.current_plan_id points at the live version → history & rollback for free

Day
  id, plan_id, date, city_hint, tz
  window_start, window_end             # e.g. 09:00–22:00 local — feasibility budget

Stop                                   # one dot on the map
  id, day_id, seq                      # ordered within the day
  place_id
  stop_kind                            # visit | meal | lodging | activity | transit
  planned_arrival, duration_min        # local time
  booking {ref, url, cost, ledger_entry_id?}   # optional
  notes

Leg                                    # computed + cached, never user-edited
  from_stop_id, to_stop_id
  mode                                 # walk | transit | drive | flight
  distance_m, duration_min
  feasibility                          # ok | tight | unreasonable | impossible (§5)
  provider_snapshot_at                 # cache timestamp
```

### 3.3 Change management: structural vs content

Two classes of change, with different rules:

**Structural changes** — anything that reshapes the route: add/remove a stop or
day, move a stop between days, reorder within a day, swap a stop's place.
These require approval: **either any leader approves, or a poll passes.**

```
Proposal                               # a structural change awaiting approval
  id, trip_id, created_by, source      # source: web | token:<id>
  title, rationale
  change_set                           # diff against a specific plan version
  route                                # leader_approval | poll
  status                               # draft | pending | approved | rejected | applied | stale
  decided_by?                          # leader user_id, or poll_id

ChangeSet
  base_plan_version
  ops[]                                # add_stop, remove_stop, move_stop, reorder, swap_place, add_day, remove_day

Poll
  id, trip_id, created_by
  kind                                 # decision | plan_change (wraps a Proposal)
  title, description
  options[] {id, label, proposal_id?}
  closes_at, quorum, allow_multi
  status                               # open | passed | failed | expired
  votes[] {user_id, option_id, at}
```

- The proposer picks the route: request leader approval (fast path) or open a
  poll (contentious/fun decisions). A leader may decline to decide and convert
  a request into a poll.
- **Leaders' own structural edits apply immediately** — recorded as an
  auto-approved Proposal, so history stays complete.
- Applying a Proposal produces a new Plan version. If the base version is
  stale (another change applied first), the proposal is flagged `stale` for
  rebase instead of silently corrupting the plan.
- `kind: decision` polls remain for non-plan questions ("which restaurant
  tonight?") — outcome recorded, nothing mutated.
- Poll mechanics (defaults, per-trip configurable): majority of votes cast,
  quorum = ⌈members/2⌉, deadline required, leaders break ties.

**Content edits** — text and metadata that don't reshape the route: titles,
descriptions, notes, planned times & durations, photos, booking info, tags,
notices. Leaders and members edit these **directly, no approval needed**;
every change lands in a field-level, revertible history:

```
Edit
  id, trip_id, entity {stop|day|candidate|notice|trip}, entity_id, field
  old_value, new_value
  author, source                       # web | token:<id>
  status                               # applied | pending_review | rejected | reverted
  created_at
```

Time/duration edits re-trigger the feasibility engine (§5) — they can flag a
day as tight/unreasonable, but flags inform rather than forbid.

**AI-originated changes** (any mutation arriving via an API token) never apply
directly. They enter the **token owner's review queue** with
`status: pending_review`: approving a content edit applies it (attributed to
the owner, labeled "via AI"); approving a structural proposal merely
*publishes* it, after which it still needs leader approval or a poll like any
human proposal. Rejection discards it. See §7.

### 3.4 Comments & discussions

```
Thread   id, trip_id, anchor {trip | day | stop | poll | candidate}, title
Comment  id, thread_id, author, body (markdown), created_at, reactions[]
```

Anchoring threads to any entity gives "discuss this restaurant" and "discuss
day 3" without separate systems.

### 3.5 Ledger

```
Expense
  id, trip_id, paid_by, amount, currency, fx_rate_to_base
  category                             # lodging | food | transport | tickets | other
  split                                # even | shares | exact  → participants[] {user_id, weight|amount}
  note, receipt_photo_key?, created_at, linked_stop_id?

Settlement
  id, trip_id, from_user, to_user, amount, settled_at
```

Balances are computed, never stored: net position per member = paid − owed −
settlements. The UI shows a **simplified debt graph** (min-cash-flow algorithm,
Splitwise-style) so five friends see "A pays B ¥3,200" instead of a web of IOUs.
Multi-currency: each expense keeps its original currency + a frozen fx rate to
the trip's base currency (rate fetched at entry time from a free API, editable).

### 3.6 Notices ("worth knowing")

```
Notice
  id, trip_id, category    # visa | safety | health | money | connectivity | packing | custom
  title, body (markdown), source_url?, pinned
  checklist_items[] {text, done_by[]}   # "buy JR Pass", "travel insurance" — per-person checkable
```

Rendered as a dedicated "Before you go" tab; pinned notices also surface on the
trip overview.

### 3.7 Users, sessions, API tokens — see §6 and §7.

---

## 4. UI design

### 4.1 Map-first layout

- **Trip view:** the whole route on one map; each day's polyline in a distinct
  color; numbered dots for stops. A horizontal day-scrubber (D1 · D2 · …)
  filters to **Day view**.
- **Day view:** that day's stops + legs only, plus a timeline rail (mobile:
  bottom sheet; desktop: left panel) listing stops in order with leg
  durations and feasibility flags in between.
- **Selection behavior:** by default, the first stop of the visible route is
  selected and its card shown. Clicking any dot opens its **Stop card**:
  - title, kind icon, photo, description/pitch
  - rating + price level, opening hours (with "closed when you arrive" warning)
  - links: website, official page, "open in Google Maps"
  - planned arrival + duration, booking ref if any
  - linked expense(s), thread ("discuss"), and "propose a change" button
- **Candidates layer:** a toggle shows shortlisted-but-unplanned candidates as
  hollow dots — the group sees what's competing for a slot.
- Map rendering goes through the `MapRenderer` interface: `setMarkers()`,
  `drawRoute()`, `fitBounds()`, `onMarkerClick()` — Google Maps JS is one
  implementation.

### 4.2 Other screens

- **Polls tab:** open polls first with countdown; plan-change polls render a
  **visual diff** (before/after mini-map + text summary of ops).
- **Ledger tab:** expense feed, per-person balance bars, "settle up" plan.
- **Before-you-go tab:** notices + checklists (§3.6).
- **Trip settings:** members, roles, poll rules, base currency, API tokens.

### 4.3 Responsive strategy

Single React app, mobile-first CSS. Map is the shell on both form factors; the
detail panel is a draggable bottom sheet on phones and a side panel ≥ 1024 px.
Ship as a **PWA** (installable, cached shell + last-loaded trip readable
offline — invaluable mid-trip with bad roaming data).

---

## 5. Feasibility engine

Runs whenever a plan version is created or a proposal is previewed, using
`RoutingEngine` (Google Routes API adapter) with aggressive caching.

- **Leg cache:** key = (from_place, to_place, mode, hour-bucket). Friend-group
  planning iterates on the same handful of places, so cache hit-rate is high —
  this is what keeps us inside the free tier.
- **Mode inference:** < 2 km → walk; same metro area → transit/drive; different
  country or > 700 km → flight (flight legs use great-circle estimate + fixed
  airport overhead of ~3 h, no paid API needed).
- **Flags per leg and per day:**
  - `ok` — day's total (visits + legs) ≤ 85 % of the day window
  - `tight` — 85–100 % of window
  - `unreasonable` — exceeds window, or arrival after closing hours
  - `impossible` — physically absurd (e.g. Shibuya morning → Seoul afternoon
    → back same day; > 16 h total travel; flight leg without airport time)
- Flags are shown inline in the day timeline and on poll previews, so voters
  see "this proposal makes Day 2 infeasible" *before* voting. Infeasible plans
  can still be saved — the app warns, it doesn't forbid (the group decides).

---

## 6. Auth: Cloudflare Access one-time PIN

Login is delegated to **Cloudflare Access** (Zero Trust, free plan covers up
to 50 users) using its **One-Time PIN** identity method: the user enters their
email on Cloudflare's login page, Cloudflare emails them the code, verifies
it, and forwards authenticated requests to our origin with a signed
`Cf-Access-Jwt-Assertion` header. We build **no OTP infrastructure at all** —
no code generation, no email sending, no bot protection for a login form
(so no Turnstile, no SES in v1).

- **Backend:** the `adapter-cf-access` implementation of `IdentityProvider`
  validates the Access JWT against our team's JWKS
  (`https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`), checks the
  `aud` tag, and maps the verified email to a user row (auto-provisioned on
  first login; display name prompted after).
- **Membership = Access policy.** Inviting a friend means adding their email
  to the Access policy (manually in the dashboard, or later automated via the
  Cloudflare API from our invite flow). There is no self-serve signup — a
  feature, not a bug, for a friends-only app.
- **API paths for AI tokens** (`/api/*` with `Authorization: Bearer itn_…`)
  get an Access *bypass* (or service-auth) policy; our backend enforces
  bearer-token auth on those routes itself (§7).
- **Origin hardening:** the Lambda Function URL only accepts requests carrying
  a secret header injected by Cloudflare, so Access can't be bypassed by
  calling the origin directly.

Known trade-offs, accepted for v1: the login page is Cloudflare-hosted (not
branded), the 50-user free cap, and coupling to Cloudflare — mitigated by the
`IdentityProvider` trait, whose documented fallback adapter is self-hosted
email OTP (hashed 6-digit codes via SES/Resend + Turnstile on the request
form) should we ever outgrow Access.

---

## 7. AI access: short-lived scoped API tokens

The goal: let ChatGPT/Claude/agents call the Itinera API *as a constrained
version of you*, without sharing your session and without paying for extra AI
API keys. Design:

- **Token model:** `ApiToken { id, user_id, name, prefix, hash, scopes[],
  expires_at, last_used_at, revoked_at }`. The plaintext token
  (`itn_<32-byte base62>`) is shown **once** at creation; the server stores
  only a SHA-256 hash (prefix kept for indexed lookup).
- **TTL:** user picks 1 h / 8 h / 24 h / 7 d (max). Expiry is enforced
  server-side; expired tokens 401. Short default (24 h) keeps leaked-token
  blast radius small.
- **Scopes:**
  - `read` — trips, plans, candidates, polls, ledger (read-only)
  - `propose` — submit content edits, candidates, structural proposals,
    decision polls, comments
  - deliberately **no** `vote`, no `admin`, no direct writes of any kind.
- **Owner review queue (the AI airlock):** every token-originated mutation is
  created with `status: pending_review` and appears in the token owner's
  review queue (§3.3). The owner approves or rejects each item; approved
  content edits apply under the owner's name (labeled "via AI"), approved
  structural proposals are merely published and still face leader approval or
  a poll. AI can research and draft; only humans commit.
- **Usage:** `Authorization: Bearer itn_…` on the same REST API. We publish
  `/openapi.json` + a short "give this to your AI" instructions page, so users
  can paste the spec + token into a ChatGPT Action / Claude tool config.
  Later: ship a tiny MCP server (`itinera-mcp`) that wraps the API, so Claude
  Code / Desktop users get first-class tools.
- **Safety rails:** per-token rate limits (e.g. 300 req/h), every mutation
  audit-logged with token id ("proposed by Kaiyu **via AI token 'claude'**" is
  shown in the UI), one-click revoke, all tokens listed with last-used time.

---

## 8. API sketch (REST, JSON)

```
GET   /me                      # identity from validated Access JWT; auto-provisions
GET   /trips                   POST /trips             GET /trips/:id
POST  /trips/:id/members       # invite = add to trip + Access policy (leader only)
GET   /trips/:id/candidates    POST /trips/:id/candidates
GET   /trips/:id/plan          GET  /trips/:id/plan/versions
GET   /plans/:id/days/:date    GET  /legs?from=&to=&mode=
PATCH /stops/:id  PATCH /days/:id  PATCH /notices/:id   # content edits — immediate, history-logged
GET   /trips/:id/history       POST /edits/:id/revert   # field-level edit log
POST  /trips/:id/proposals     # structural ChangeSet → leader approval or poll
POST  /proposals/:id/approve   POST /proposals/:id/to-poll        # leader actions
POST  /trips/:id/polls         POST /polls/:id/votes   POST /polls/:id/close
GET   /me/review-queue         POST /review/:id/approve|reject    # AI airlock (§7)
GET|POST /threads/:id/comments
GET   /trips/:id/ledger        POST /trips/:id/expenses  POST /trips/:id/settlements
GET   /trips/:id/notices       POST /trips/:id/notices
GET|POST|DELETE /me/tokens
GET   /openapi.json
```

Same API for browser (Access JWT) and AI (bearer token); middleware resolves
either into an authenticated principal with scopes (browser identities get all
scopes; token mutations are diverted into the review queue).

---

## 9. Map provider notes (Google, behind interfaces)

- SKUs used: Maps JavaScript (render), Places Text Search + Details + Photos
  (catalog), Routes / Distance Matrix (legs). Essentials tier free monthly
  call allowances (~10 k/SKU) are ample at friend scale **because we cache**:
  place details and photos are fetched once at candidate-creation and stored
  in our DB/R2; legs are cached per §5.
- **ToS caveat to verify before launch:** Google's terms restrict long-term
  caching/storage of some Places content (photos, reviews). If it's a
  problem, the `PlaceCatalog` adapter falls back to storing only
  `place_id` + refreshing on view — this is exactly why the trait exists.
- API key hygiene: browser key HTTP-referrer-locked + Map-render-only;
  server key IP-locked to Lambda egress, holds Places/Routes quotas.

## 10. Cost budget (monthly, friend-group scale)

| Component                        | Tier                        | Cost   |
|----------------------------------|-----------------------------|--------|
| AWS Lambda + Function URL        | 1 M req/mo always-free      | $0     |
| Neon Postgres                    | free tier (0.5 GB, autosuspend) | $0 |
| Cloudflare Pages / DNS / WAF     | free                        | $0     |
| Cloudflare Access (OTP login)    | Zero Trust free, ≤ 50 users | $0     |
| Cloudflare R2 (photos)           | 10 GB free                  | $0     |
| Amazon SES (v2 digests, optional)| $0.10 / 1 000 emails        | ~$0    |
| Google Maps Platform             | Essentials free allowances + caching | $0 |
| Domain (itinera.*)               | —                           | ~$10/yr |

The only structural risk is Google Maps overage; mitigations: caching (§5, §9),
per-key quota caps set to free-tier limits (hard stop, no surprise bills), and
the `MapRenderer`/`PlaceCatalog`/`RoutingEngine` interfaces as the escape hatch.

---

## 11. Things your spec didn't mention (recommended additions)

Included in this design:

1. **Time zones** — every Day/Stop time is local; cross-country trips break
   without this (§3.2).
2. **Plan versioning & rollback** — free consequence of approval-gated
   structural ChangeSets, plus field-level edit history with revert for content.
3. **Opening-hours warnings** — "you arrive 40 min after last entry" (§5).
4. **Multi-currency ledger + debt simplification** (§3.5).
5. **Poll mechanics** — deadlines, quorum, tie-breaks, stale-proposal rebase (§3.3).
6. **Audit trail for AI actions** — provenance shown in UI (§7).
7. **PWA/offline** — read your plan with no roaming data (§4.3).
8. **Booking info on stops**, linkable to ledger expenses (§3.2).
9. **Candidates layer on the map** — see what's competing, not just what won (§4.1).
10. **Per-person checklists** inside notices (§3.6).

Deferred (v2+ candidates, deliberately not in v1):

- Calendar export (ICS) and email digests of new polls/comments.
- Weather forecast on days near the trip date.
- Read-only public share link ("send mom the itinerary").
- Real-time presence/live cursors (WebSockets don't fit Lambda free tier well;
  polling every 30 s is fine for v1).
- MCP server for first-class Claude/agent integration (§7).
- Photo albums / post-trip journal — Itinera already knows where you were.

## 12. Build order (suggested milestones)

1. **Skeleton:** monorepo, CI, `cargo-lambda` deploy, Cloudflare Access auth
   (JWT validation + auto-provisioning), trip CRUD, members & roles.
2. **Map core:** place search via `PlaceCatalog`, candidates, plan/day/stop model,
   map + day views with stop cards.
3. **Feasibility:** `RoutingEngine` adapter, leg cache, flags in UI.
4. **Governance:** content edits + edit history/revert, structural proposals,
   leader approval, polls, comments/threads.
5. **Money:** ledger, balances, settle-up.
6. **AI door:** API tokens, scopes, OpenAPI publishing, audit trail.
7. **Polish:** notices, PWA/offline, invites, rate limits, quota caps.
