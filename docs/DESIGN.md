# Itinera — Design Document

Status: draft v1 · 2026-08-02 · author: Kaiyu Huang + Claude

Itinera is a collaborative trip planner for a small group of friends. A trip is a
multi-day route drawn on a map; the group proposes candidate places, votes on
changes through polls, discusses in threads, splits costs in a shared ledger, and
can let AI assistants participate via short-lived scoped API tokens.

---

## 1. Guiding principles

1. **Everything behind an interface.** Every external dependency — maps, routing,
   email, database, object storage — is accessed through a Rust trait (backend) or
   TypeScript interface (frontend). Callers never import a vendor SDK directly.
   Swapping Google Maps for MapLibre/OSRM, or DynamoDB for another durable
   store, must not touch business logic.
2. **Two-tier governance, everything historied.** _Structural_ changes (anything
   that reshapes the route: add/remove/move stops or days) require a poll **or**
   a leader's approval. _Content_ edits (text, times, photos, bookings) apply
   immediately but are recorded in a field-level, revertible edit history.
   AI-originated changes of either kind are staged for the token owner's
   personal review before they enter the system at all.
3. **As close to $0/month as possible.** Every component is chosen to fit a
   permanent free tier at friend-group scale. See §10 for the cost table.
4. **Phone-first viewing, laptop-first editing.** The people on the trip will
   mostly _read_ the plan on a phone; heavy editing happens beforehand on a laptop.

---

## 2. Architecture overview

```
 ┌────────────────────────────┐
 │  Frontend (React + TS)     │  Cloudflare Pages (free, CDN, custom domain)
 │  MapView / DayView / Polls │
 └────────────┬───────────────┘
              │ HTTPS (JSON API; Access-authenticated caller)
 ┌────────────▼───────────────┐
 │  Cloudflare Access + Worker│  OTP/service admission, TLS, edge proof
 └────────────┬───────────────┘
              │ Access JWT + proof
 ┌────────────▼───────────────┐
 │  Amazon CloudFront         │  proof Function, no API cache, OAC SigV4
 └────────────┬───────────────┘
              │ AWS_IAM
 ┌────────────▼───────────────┐
 │  AWS Lambda (Rust, axum)   │  single "monolith" function via Function URL
 │  ┌───────────────────────┐ │  (Function URLs are free — no API Gateway cost)
 │  │ domain core (traits)  │ │
 │  └──┬──────┬──────┬──────┘ │
 └─────┼──────┼──────┼────────┘
       │      │      │ adapter crates (one per provider)
   DynamoDB  Google  Cloudflare R2
   (AWS)     Maps    (photos, free 10 GB)
```

- **One Lambda, not microservices.** A single axum app compiled with
  `cargo-lambda` + `lambda_http`. At this scale, splitting functions only adds
  cold starts and deployment complexity.
- **Lambda Function URL instead of API Gateway.** The Function URL uses
  `AWS_IAM` and grants invocation only to one CloudFront distribution.
  CloudFront OAC signs origin requests; a viewer Function first validates and
  removes a proof injected by the Access-protected Worker. Invalid public
  traffic therefore stops before Lambda. API caching is disabled, Rust still
  validates every Access assertion, and budgets and concurrency limits cover
  the remaining cost risk. See [the trusted request journey](SECURITY.md#the-journey-of-a-trusted-request).
- **Database: one DynamoDB table in AWS.** Lambda reaches it with its execution
  role, so production needs no database password, connection pool, VPC, or
  third provider account. Trip aggregates share a partition, while condition
  expressions and transactional writes enforce uniqueness, version checks,
  governance decisions, and their audit records. Repository traits keep the
  physical key design out of the domain. The complete access-pattern and
  invariant design lives in [`DYNAMODB.md`](DYNAMODB.md).

### 2.1 Ports (traits) — the swappability contract

```rust
// backend/crates/core/src/ports/
trait PlaceCatalog   { fn search(&self, q) -> …; fn details(&self, ref) -> …; fn photo_url(&self, ref) -> …; }
trait RoutingEngine  { fn leg(&self, from, to, mode, depart) -> Leg; fn matrix(&self, pts, mode) -> …; }
trait IdentityProvider { fn authenticate(&self, req) -> Identity;   // Cloudflare Access JWT adapter;
                         fn grant_login(&self, email) -> …;         // fallback adapter: self-hosted OTP + SES
                         fn revoke_login(&self, email) -> …; }      // grant/revoke = Access policy via CF API
trait Mailer         { fn send_digest(&self, …) -> …; }              // v2 only (digests/invites) — login needs no email from us
trait BlobStore      { fn put(&self, key, bytes) -> Url; }
trait Clock / IdGen  // deterministic tests

// repositories
trait TripRepo, PlanRepo, PollRepo, LedgerRepo, UserRepo, TokenRepo, CommentRepo
```

Adapters: `adapter-gmaps` (implements `PlaceCatalog` + `RoutingEngine` with
Places API + Routes API), `adapter-cf-access`, `adapter-dynamodb`, `adapter-r2`.
The frontend mirrors this with a `MapRenderer` interface implemented by
`GoogleMapRenderer` (and later, potentially, `MapLibreRenderer`).

### 2.2 Repository layout (monorepo)

```
itinera/
├── backend/                 # Rust workspace
│   ├── crates/core/         # domain types, ports, services (no vendor deps)
│   ├── crates/adapters/     # DynamoDB, Cloudflare Access, gmaps, ses, r2
│   └── crates/api/          # axum routes, auth middleware, lambda entrypoint
├── frontend/                # Vite + React + TypeScript
├── edge/                    # TypeScript Cloudflare Worker and edge-gate tests
├── infra/                   # Terraform *module* — values injected by the private deploy repo (§2.3)
└── docs/
```

### 2.3 Two repos: public app, private deploy

Everything that _is_ the app is public; everything that _points at a real
deployment_ is private.

- **`itinera` (public, this repo)** — application code, docs, the API
  contract, CI (application checks plus mocked infrastructure tests), and infra
  _code_: `infra/` is a backend-free Terraform child module. Real deployment
  values are required inputs rather than defaults. The workflow token is
  read-only and no secret or AWS credential is configured here. Public Actions
  logs are world-readable, so CI runs formatting, provider-schema validation,
  and mock-provider tests only—never an environment plan or apply.
- **`itinera-deploy` (private)** — the Terraform _root_ module: real
  `terraform.tfvars`, remote state backend config, GitHub environment
  secrets, deployment URLs, ops notes, and the deploy workflow, which checks
  out `itinera` at a pinned tag/commit, builds, and applies. Triggered
  manually (`workflow_dispatch` with a git ref) or on a release tag.

Which side of the line a value lands on:

| Public (`itinera`)                                          | Private (`itinera-deploy`)                                                    |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Terraform module code and resource naming rules             | `terraform.tfvars`: prefixes, account IDs, ARNs                               |
| IAM policies (least-privilege — they are a public map)      | custom domain, zone ID, Access hostname — anything that reveals the app's URL |
| Required input declarations with no real values             | deployment URLs, Worker proof, Access bindings, ops runbook                   |
| `.env.development` (`VITE_API_BASE_URL=http://localhost:…`) | production `VITE_API_BASE_URL`, injected at build time                        |
| —                                                           | short-lived GitHub OIDC deployment role and managed Worker/runtime secrets    |

Terraform **state lives in neither repo**. The private root uses an encrypted S3
backend with native locking (`use_lockfile = true`). Its separately bootstrapped
bucket has versioning and all public-access blocks enabled, and the OIDC deploy
role is scoped to the application state and lock objects. State contains every
resolved value and sometimes plaintext secrets, and public git history is
forever. The public child module therefore has no `backend` or `provider` block.
Its dependency lock makes public validation reproducible, while the private root
keeps the authoritative deployment lock file.

Note the two distinct boundaries. The split keeps real identifiers out of
public _source and logs_, but the deployed frontend bundle necessarily bakes
in the API base URL (`VITE_*` vars are substituted at build time), so anyone
who can load the app can read it. The actual security boundary is Cloudflare
Access in front of both the Pages site and the API (§6); URL privacy is
drive-by-discovery hygiene, never an auth mechanism.

---

## 3. Data model

Hierarchy: **Trip → Plan (versioned) → Day → Stop**, with a shared **Place**
catalog underneath and **Candidates** owning editable place snapshots for a
specific trip.

### 3.1 Places & candidates

```
Place                          # catalog entry or candidate-owned snapshot
  id, name, kind               # kind: sight | food | lodging | activity | transport_hub
  lat, lng, tz                 # IANA timezone, resolved once at import
  country_code, admin_area, city, address
  external_ref                 # e.g. {provider: "google", place_id: "…"} — behind PlaceCatalog
  website, phone, rating, price_level, opening_hours (cached JSON)
  photo_keys[]                 # R2 keys; photos cached to our storage (see §9 ToS note)
  guide                        # nullable editorial context: summary, intro,
                               # activity ideas {title, details?}, and practical tips

Candidate                      # "shortlist" of a trip — the pool polls choose from
  id, trip_id, place_id        # points to its editable place snapshot
  source_place_id              # nullable catalog lineage; provider facts are inherited
  proposed_by, created_at
  pitch                        # why this place — free text
  tags[]                       # "must-see", "rainy-day", "splurge"…
  status                       # shortlisted | in_plan | rejected
```

The UI calls Candidates **Trip ideas**: an optional pool the group can consider,
not a checklist or a commitment. Picking a catalog result creates a
candidate-owned Place snapshot. Provider facts such as coordinates, rating,
price and external reference are inherited and read-only; members can edit the
snapshot's name, kind, city, address, contact details, opening-hours copy,
photos and guide. Manual ideas create the same snapshot without catalog
lineage. Editing an idea forks its snapshot again, so it never rewrites the
catalog, another candidate, or a stop already adopted by the plan.

Catalog `Place.guide` content is app-curated. A candidate snapshot's guide is
member-authored context for that idea. `Candidate.pitch` remains the proposer's
trip-specific reason, while `Stop.notes` remains itinerary-specific; none of
these silently becomes another layer's copy.

An activity idea always has a short title and may have explanatory `details`.
The title is the scannable pool entry; details answer questions such as what the
activity involves or why it is useful. They are omitted rather than filled with
placeholder copy when there is nothing more to say. Editing a stop or proposing
a structural plan change never exposes or rewrites either the catalog guide or
the candidate snapshot's guide.

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

Plan                                   # a full itinerary; v1 is bootstrapped from the first placed idea
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

A trip may exist without a Plan while the group is only gathering ideas. The
first **Propose for a day** action bootstraps Plan v1: it creates one empty Day
for every date in the trip and seeds each Day's `city_hint` / `tz` from that
idea's candidate-owned Place snapshot. The action is idempotent and does not
adopt the idea by itself; the stop still goes through the normal leader or poll
route. Every Plan version after v1 is created only by an applied Proposal.

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
  quorum = ⌈members/2⌉, deadline required. A tied top result closes as
  `failed` with no decision; it never selects an option by storage order and
  never applies a structural proposal. The group can open a fresh poll.
- A passing `plan_change` poll still applies through the same proposal boundary
  as direct leader approval. If its `base_plan_version` is no longer current,
  the proposal becomes `stale`, the poll closes `failed` with an explanation,
  and no Plan version or structural data is changed.

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
_publishes_ it, after which it still needs leader approval or a poll like any
human proposal; approving a candidate adds it to that trip's idea pool; and
approving a comment publishes it to its discussion thread. Every review item
carries trip context, and structural decisions stay locked until the relevant
plan preview has loaded. Rejection discards it. See §7.

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
  id, trip_id, created_by, category    # visa | safety | health | money | connectivity | packing | custom
  title, body (markdown), source_url?, pinned
  status                  # active | resolved | archived
  checklist_items[] {text, done_by[]}   # "buy JR Pass", "travel insurance" — per-person checkable
```

Rendered as a dedicated "Before you go" tab; pinned notices also surface on the
trip overview. A notice's author and trip leaders may edit, pin, resolve,
archive, or restore it; other members can read it, use source links, and update
their own checklist state. Archive is reversible: archived notices leave active
counts and the main list, but remain available through an explicit archived
view. User-authored titles, bodies, checklist copy, and source URLs are stored
and rendered unchanged by the UI locale.

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
  - title, kind icon, photo, guide summary, and trip-specific note/pitch
  - an optional activity pool labelled **Ideas while you're here**; each idea
    shows its title, and ideas with details have an inline disclosure button
  - rating + price level, opening hours (with "closed when you arrive" warning)
  - links: website, official page, "open in Google Maps"
  - planned arrival + duration, booking ref if any
  - linked expense(s), thread ("discuss"), and "propose a change" button
- **Candidates layer:** a toggle shows shortlisted-but-unplanned candidates as
  hollow dots — the group sees what's competing for a slot.
- Map rendering goes through the `MapRenderer` interface: `setMarkers()`,
  `setRoutes()`, `fitBounds()`, `onMarkerClick()`, and `setUiLabels()` for
  provider-owned controls — Google Maps JS is one implementation. Localized
  zoom/attribution labels update in place when the UI language changes; the map
  is not rebuilt or reset.

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

Cover photographs never serve as an unprotected text background. The trip
hero uses a neutral, content-owned scrim that grows with wrapped titles and
localized metadata, keeping white title/meta copy readable without flattening
the whole photograph.

The timeline repeats the trip ribbon's environmental vocabulary in its clock
rail: a shared solar ramp plus sun/cloud shapes for daylight and moon/star
shapes for night. The itinerary cards remain on a neutral planning surface;
hour labels carry their own readable substrate, and the decorative sky scene is
hidden from assistive technology. The labelled sunrise/sunset markers are the
accessible statement of the transition times.

### 4.4 Place-guide disclosure

Place guides use progressive disclosure so a stop remains easy to scan:

- The summary and activity titles are visible without interaction. A compact
  card may show only the first few titles, with the complete guide available
  in the selected-stop panel or details sheet.
- A plus next to an activity is a real button, never a decorative bullet. It
  appears only when that activity has optional details, toggles those details
  inline, exposes `aria-expanded` / `aria-controls`, and changes to a minus (or
  equivalent expanded-state icon) while open. Its accessible name includes the
  activity title. Activities without details have no disclosure control.
- Expanded text is independent per activity and does not imply that the group
  selected or committed to it. The pool language remains **Pick what fits**,
  not a fixed itinerary checklist.
- The Trip idea create/edit form owns the candidate snapshot's guide. An
  activity title is required and its details field is optional; blank details
  are omitted. Trip proposal and stop-edit forms continue to edit the
  trip-specific pitch/note rather than exposing either place-guide source by
  accident.
- Full guides opened from maps or compact cards use the established desktop
  dialog / mobile bottom sheet and remain explicitly dismissible. Expanding an
  activity must not move the user to another tab.

### 4.5 Action hierarchy and colour

Buttons use colour to express hierarchy, not decoration:

- Each action group has at most one primary action. **Propose change** (or the
  form's submit action) is a solid `--accent` button with
  `--accent-contrast` text; hover/pressed treatment derives from
  `--accent-strong`.
- Secondary actions such as **Discuss** use the surface, standard border, and
  normal text tokens. Tertiary actions such as an overflow menu or inline
  disclosure use quiet icon/link treatment and retain a visible focus ring.
- Destructive confirmation actions use `--color-impossible`; they do not reuse
  the trip accent. Disabled states reduce emphasis but keep their label
  readable. Text, icons, and confirmation copy carry meaning independently of
  colour.
- The hierarchy is consistent in timeline inspectors, map cards, dialogs, and
  mobile sheets. A wide primary button must not fall back to a white fill just
  because the layout changes at a breakpoint.

### 4.6 Language

The interface can switch between English (`en`) and Simplified Chinese
(`zh-CN`) without a reload. The selector lives in shared app chrome, applies to
every route and modal, and the chosen locale is a device-local preference. On a
device with no saved preference, Simplified-Chinese browser locales select
`zh-CN`; all other and unsupported locales fall back to English.

This is **UI localisation only**. It covers navigation, buttons, field labels,
helper and validation text, empty/error states, built-in kind/status labels,
date/number formatting, and accessibility names. User-authored values — for
example trip and place names, candidate pitches, stop notes, poll text,
comments, expenses, and prep notices — are rendered exactly as stored and are
never translated or rewritten when the locale changes. Provider and editorial
place content (`Place.guide`) likewise remains the stored source text unless a
separate content-localisation contract is introduced later.

The client owns the message catalogue and locale preference; requests do not
send a locale and the API does not persist one. Components receive translated
UI labels through the shared localisation layer rather than embedding English
strings. Tests cover both locales, fallback behaviour, persistence, and the
invariance of user-authored content.

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
  see "this proposal makes Day 2 infeasible" _before_ voting. Infeasible plans
  can still be saved — the app warns, it doesn't forbid (the group decides).

---

## 6. Auth: Cloudflare Access one-time PIN

The complete trust-boundary, threat-model, authorization, and deployment design
lives in [`SECURITY.md`](SECURITY.md). This section is the product-level summary.

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
  `aud` tag, and resolves the verified email claim to a stable user profile
  (both auto-provisioned atomically on first login; display name prompted
  after). The profile is keyed by opaque `user_id`, so a future verified email
  change replaces the lookup claim without changing identity references.
  Production runtime configuration
  provides the public team origin through `ITINERA_CF_ACCESS_TEAM_DOMAIN` and
  the application audience tag through `ITINERA_CF_ACCESS_AUDIENCE`. Local
  development may opt into the deliberately insecure email-as-assertion
  adapter only when the backend is compiled with `--features dev-auth` **and**
  `ITINERA_DEV_AUTH_ENABLED=1`; default production builds contain neither that
  adapter nor the in-memory repository, and development is never an implicit
  fallback when production configuration is missing.
- **Membership = Access policy, fully automated.** Inviting a friend is one
  click in the app: a leader enters an email → the backend calls
  `IdentityProvider::grant_login`, whose Cloudflare adapter adds the email to
  the Access policy via the Cloudflare API, records an
  `Invite {email, trip_id, invited_by, status: pending}`, and returns a link
  to send the friend. They open the link, Cloudflare emails them the code,
  and on first login the pending invite converts into trip membership.
  Nobody ever touches the Cloudflare dashboard. There is still no open
  self-serve signup — you must be invited by an existing member — which is
  the right gate for a friends-only app.
- **Revocation:** removing someone from a trip removes the membership; their
  email is revoked from the Access policy (`revoke_login`) only when they
  belong to no other trip, since Access grants app-wide login rather than
  per-trip access. Trip-level authorization is always enforced by the
  backend regardless.
- **Approved automation** authenticates with specifically named Cloudflare
  Access service tokens. Access emits the same application assertion envelope;
  Rust resolves its `common_name` through a separate, pre-created service
  mapping with narrow owner and trip scopes (§7). There is no Access bypass.
- **Origin hardening:** the Worker replaces a high-entropy proof, a CloudFront
  viewer Function validates and removes it, OAC signs the origin request, and
  the Lambda Function URL accepts only that distribution through `AWS_IAM`.

Known trade-offs, accepted for v1: the login page is Cloudflare-hosted (not
branded), the 50-user free cap, and coupling to Cloudflare — mitigated by the
`IdentityProvider` trait, whose documented fallback adapter is self-hosted
email OTP (hashed 6-digit codes via SES/Resend + Turnstile on the request
form) should we ever outgrow Access.

---

## 7. AI access: short-lived scoped API tokens

> **Superseded contract.** The custom `itn_…` bearer-token design below remains
> in the frozen frontend/OpenAPI mock and has not been implemented in Rust. It
> will be replaced by the Cloudflare Access service-identity model described in
> [`SECURITY.md`](SECURITY.md#being-signed-in-is-not-being-invited). The origin
> path in this document does not permit an Access bypass.

The goal: let ChatGPT/Claude/agents call the Itinera API _as a constrained
version of you_, without sharing your session and without paying for extra AI
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
  a poll, approved candidates enter the trip's idea pool, and approved comments
  publish to their thread. AI can research and draft; only humans commit.
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
POST  /trips/:id/invites       # leader only: grants Access login + pending invite (§6)
DELETE /trips/:id/members/:uid # removes membership; revokes login if last trip
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

| Component                         | Tier                                          | Cost        |
| --------------------------------- | --------------------------------------------- | ----------- |
| AWS Lambda + Function URL         | 1 M req/mo always-free                        | $0          |
| Amazon DynamoDB                   | provisioned free tier, 25 GB storage          | $0          |
| Cloudflare Pages / DNS            | free                                          | $0          |
| CloudFront Function + OAC         | pay-as-you-go now; $0 flat-rate plan targeted | $0 expected |
| Cloudflare Access (OTP login)     | Zero Trust free, ≤ 50 users                   | $0          |
| Cloudflare R2 (photos)            | 10 GB free                                    | $0          |
| Amazon SES (v2 digests, optional) | $0.10 / 1 000 emails                          | ~$0         |
| Google Maps Platform              | Essentials free allowances + caching          | $0          |
| Domain (itinera.*)                | —                                             | ~$10/yr     |

The only structural risk is Google Maps overage; mitigations: caching (§5, §9),
per-key quota caps set to free-tier limits (hard stop, no surprise bills), and
the `MapRenderer`/`PlaceCatalog`/`RoutingEngine` interfaces as the escape hatch.
The DynamoDB estimate assumes the Standard table class with provisioned
capacity inside the per-Region, per-payer-account free allowance. Optional
point-in-time recovery is separately billed and deliberately retained as a
production safety cost.

Before production cutover, migrate the distribution to CloudFront's **$0 Free
flat-rate plan** when the AWS account is eligible. That migration must replace
the Free tier's unsupported custom cache, origin-request, and response-header
policies with reviewed AWS-managed policies while preserving exact forwarding,
`private, no-store`, and fail-closed guarantees in the Worker and Rust API. The
private deployment then attaches a dedicated, non-shared plan-provided WAF with
IP rate limiting, subscribes the distribution, and repeats direct-CloudFront and
direct-Lambda negative smoke tests. The Worker proof, CloudFront Function, OAC,
Lambda IAM boundary, concurrency limits, and budgets remain in place. If the
account is not eligible, deployment stays on pay-as-you-go with its free
allowances and alarms rather than weakening these controls.

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

## 12. Implementation plan

**Who builds what:** Claude writes the frontend; Kaiyu writes the backend while
learning Rust and axum. The complete frontend was built first against mock data.
The Rust workspace, Access authentication, user provisioning, DynamoDB user
repository, public AWS module, and protected CloudFront origin are also complete.

**How the two halves meet:** the frontend never calls `fetch` directly — it
talks to an `ApiClient` TypeScript interface (interface-first, as everywhere).
During Phase A its implementation is `MockApiClient`, backed by rich fixtures
(a realistic multi-city Japan trip: candidates, a 7-day plan, open polls,
pending AI edits, a ledger with debts). Freezing the frontend means freezing
`ApiClient` — at that point it is exported as `docs/openapi.yaml` + the fixture
set, and that contract is the backend's spec. Before further backend work, the
small amount of contract drift introduced by later UI improvements is reconciled.
Phase B then ends by swapping `MockApiClient` for `HttpApiClient`; no other
frontend feature redesign should be needed.

### Phase A — frontend on mock data (complete)

1. **Scaffold:** Vite + React + TS, routing, design tokens, PWA shell,
   `ApiClient` interface + `MockApiClient` + fixtures.
2. **Map core:** `MapRenderer` interface + the keyless `MockMapRenderer`,
   trip/day views, day scrubber, stop cards, candidates layer.
3. **Governance UI:** content editing with history/revert, structural
   proposals with visual diff, polls & voting, AI review queue.
4. **Money & prep:** ledger (expenses, balances, settle-up), notices +
   checklists, comments/threads.
5. **Polish & freeze:** responsive bottom sheet, offline, feasibility flags
   rendering, a11y pass → export `openapi.yaml` + fixtures as the contract.

### Phase B — real application and launch

The remaining work completes and tests the application locally before creating
the private cloud environment. The order is application backend → supporting
features → integrations → frontend cutover → production hardening → deployment
→ real data and launch.

1. **Reconcile the contract:** make `ApiClient` and `openapi.yaml` agree,
   including trip-status changes and expense correction/deletion, then freeze
   the production HTTP surface.
2. **Core application + DynamoDB:** implement trips, members, invites,
   candidates, plans, days, and stops; add access-pattern-led repositories,
   conditional writes, and membership/role authorization for every operation.
3. **Complete the product domain:** implement content history and revert,
   proposals, polls, discussions, ledger and settlements, notices and
   checklists, service identities, scoped API tokens, the review queue, and
   `/openapi.json`.
4. **Add integrations:** implement the Google-backed `PlaceCatalog`,
   `RoutingEngine`, and map renderer; add the leg cache and feasibility engine,
   R2 photo uploads, and the Cloudflare invite adapter. SES digests remain
   optional and must not block launch.
5. **Connect the real frontend:** implement `HttpApiClient`, production error
   handling, restrictive CORS/preflight behavior, and contract tests; switch
   production away from `MockApiClient` while retaining mock mode for local UI
   work; run the full desktop/mobile suite against the real API boundary.
6. **Prepare production hardening:** make CloudFront compatible with the $0
   Free flat-rate plan and its dedicated included WAF/rate limit; finish
   budgets, alarms, quotas, concurrency and DynamoDB capacity limits, backup
   settings, redaction, and automated security assertions. Confirm account
   eligibility, with pay-as-you-go as the documented fallback.
7. **Create the private environment and verify it:** bootstrap encrypted remote
   state and GitHub OIDC, deploy Pages, Access, the Worker, CloudFront, Lambda,
   and DynamoDB, install managed secrets, and activate the selected CloudFront
   plan. Then run the live-only checks: direct CloudFront and Lambda denial,
   invalid/expired identity rejection, cross-trip isolation, alarms, restore,
   and rollback. These checks necessarily follow resource creation even though
   their controls and test procedures are prepared in step 6.
8. **Seed and launch:** create or import the first real trip, invite the initial
   travellers, run the complete authenticated user journeys on desktop and
   mobile, promote the reviewed versions, and watch errors, throttling, and cost
   during the first real sessions.
