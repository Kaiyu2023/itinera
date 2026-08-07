# Itinera SQLite design

Status: accepted target persistence contract · 2026-08-07

[`DESIGN.md`](DESIGN.md) defines the product model and
[`adr/0001-single-node-sqlite.md`](adr/0001-single-node-sqlite.md) selects the
single-node deployment. This document defines how repository ports will persist
that model in SQLite.

The former DynamoDB/Lambda backend was removed before the SQLite adapters were
built and is preserved on the `codex/dynamodb-archive` branch. No private
environment or live production data exists, so this is a clean implementation
rewrite rather than a live database conversion. There is no persistence-backed
runtime binary until the required SQLite capability and startup gates pass.
Numeric target ceilings introduced below are migration requirements, not claims
about the archived adapter. The capability PR that enforces each ceiling
must add the matching OpenAPI `maxItems`/byte extension and contract tests; this
architecture-only PR does not claim runtime behavior that does not yet exist.

## 1. Goals and non-goals

The SQLite design must make the code smaller without weakening the semantics
already reviewed in the domain and HTTP contract.

Goals:

- use foreign keys, unique constraints, checks, indexes, and transactions for
  relational work that the former adapter expressed in application code;
- retain direct-membership authorization, trip scoping, stale-state checks,
  audit provenance, and idempotency;
- keep one repository module per capability;
- make tests run against real temporary database files and concurrent
  connections; and
- support one process on one host with simple backup and restore.

Non-goals:

- multi-node SQLite, a network filesystem, read replicas, or seamless failover;
- a generic entity/attribute/value store or a SQL version of the one-table
  DynamoDB layout;
- runtime dual writes or bidirectional DynamoDB synchronization;
- changing the OpenAPI shapes or authorization roles; or
- creating the private environment in a persistence PR.

## 2. Database ownership and connection contract

One `SqliteDb` owns a bounded `sqlx::SqlitePool` and is shared by the repository
adapters. It contains only pool setup, migrations, transaction entry points,
and small mechanical codecs. Capability SQL remains beside its repository.

Production opens exactly one database file on the local EBS filesystem. Exactly
one long-running Itinera application process owns domain reads and writes; no
second API replica may open it. A systemd-controlled one-shot backup command may
open the source only to use SQLite's Online Backup API, and migrations run only
while the app is stopped. The initial app pool maximum is four connections,
which permits concurrent readers while SQLite still serializes writes. It is a
configuration error to point at an in-memory database or a network filesystem.

The release binary statically bundles the Cargo.lock-pinned SQLite engine; it
must not dynamically use the host library. The minimum accepted upstream
version is 3.51.3, which contains the WAL-reset corruption fix. API, migration,
and backup entry points report and verify `sqlite_version()` and
`sqlite_source_id()` from that same build. Startup and readiness fail closed on
an older or unexpected engine, and CI exercises concurrent writers and
checkpoints against the bundled runtime.

Every connection applies and verifies:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA wal_autocheckpoint = 1000;
```

`journal_mode` is persistent but is still verified at startup. Failure to
enable any required setting prevents readiness. A small journal-size limit may
be selected after measurement; correctness does not depend on a particular
checkpoint threshold.

Normal startup validates the migration checksum and expected schema version but
does not apply migrations. The deploy step runs migrations explicitly before
the new service starts.

## 3. Storage conventions

- Every application table is created `STRICT`. Every required column, every
  primary-key component, and every child-side foreign-key component is
  explicitly `NOT NULL`; a column is nullable only where this document calls it
  optional. This avoids SQLite's flexible typing and legacy composite-primary-
  key null behaviour becoming part of the data model.
- Opaque IDs are non-empty `TEXT`. A trip-owned object uses a composite primary
  or unique key beginning with `trip_id`; a child ID is never loaded globally
  and then used to infer its trip.
- Every top-level trip-owned row has an explicit foreign key from `trip_id` to
  `trips`; every child uses the full composite parent key. Actor, author, owner,
  inviter, payer, participant, sender, recipient, and completer IDs reference
  stable `users`. The default delete action is `RESTRICT`; the product archives
  aggregates and tombstones identities instead of cascading away provenance.
  Any narrowly approved child cascade is named in its capability migration and
  tested. Raw foreign-trip/orphan inserts must fail at the database boundary.
- UTC instants are canonical RFC 3339 `TEXT` ending in `Z`. Dates and local times
  use the canonical API formats. Rust validates them before binding and after
  reading.
- Booleans are `INTEGER NOT NULL CHECK (value IN (0, 1))`.
- Revisions are signed SQLite `INTEGER NOT NULL CHECK (revision BETWEEN 1 AND
9223372036854775807)`. Rust performs checked conversions at the repository
  boundary and checked increment before CAS; an existing `u64` value above
  `i64::MAX` fails closed rather than wrapping or rebinding. They remain part of
  stale-write semantics even though there is one SQLite writer.
- Enumerations use `TEXT` plus an explicit `CHECK` list. Unknown stored values
  fail closed in both SQL and strict Rust decoding.
- Monetary amounts and exchange rates use constrained `REAL` columns for parity
  with the current domain types. Existing validation and half-away-from-zero
  rounding remain authoritative. A future decimal-domain change is a separate
  API and migration decision.
- JSON is canonical `TEXT CHECK (json_valid(value))`. It is allowed for bounded
  value objects, immutable ChangeSets, and audit before/after snapshots. Fields
  used for authorization, ownership, state transitions, uniqueness, expiry, or
  joins are relational columns, never hidden only inside JSON.
- Historical rows reference stable users, not live memberships. Removing a
  member must not erase their authorship, votes, expenses, or audit history.
- Foreign keys are immediate by default. The few circular publication links use
  `DEFERRABLE INITIALLY DEFERRED` and are tested with real commits.

## 4. Schema by capability

The lists below freeze table ownership, keys, and important constraints. The
versioned migration files will contain the full column DDL and indexes. Column
names may be expanded for implementation detail, but changing an ownership key
or invariant requires updating this document in the same PR.

### 4.1 Users and login claims

`users`

- `id` primary key;
- canonical email, optional display name, and profile revision;
- the profile keeps the raw email because the product must display it; routine
  logs and keys do not.

`user_email_claims`

- `email_digest` primary key;
- `user_id` unique foreign key to `users`;
- the digest is SHA-256 of the canonical email and is lookup minimization, not
  encryption.

First login inserts profile and claim in one transaction. A duplicate digest
resolves the existing user; a crossed claim/profile pair is corruption. Trip
membership counts are derived with `COUNT`, not stored on `users`.

### 4.2 Trips, memberships, and invitations

`trips`

- `id` primary key;
- name, lifecycle status, dates, base currency, presentation fields, optional
  soft budget, current plan pointer, created time, and revision;
- date and currency checks mirror OpenAPI limits;
- the current plan ID/version pair is either entirely null or entirely present;
- a composite foreign key `(id, current_plan_id, current_plan_version)` names
  the unique `(trip_id, id, version)` key on exactly one `plans` row and is
  `DEFERRABLE INITIALLY DEFERRED` for publication.

`trip_memberships`

- primary key `(trip_id, user_id)`;
- foreign keys to `trips` and `users`;
- role check for `leader | member | viewer`, join time, and revision;
- indexes `(user_id, trip_id)` for navigation and `(trip_id, role)` for role
  queries.

`trip_invites`

- primary key `(trip_id, email_digest)` and a unique server invite ID within the
  trip;
- canonical email, inviter user ID, status, creation time, and revision;
- index `(email_digest, status, trip_id)` replaces the mirrored invitee
  partition.

The creator and first leader membership commit together. Removing or demoting
a leader checks inside the same write transaction that another leader remains.
The application does not trust the user-to-trip index for authorization; every
operation queries `(trip_id, actor_id)` directly.

Capacity is explicit: a trip may have at most 1,000 current members plus pending
invitees, and a canonical user/email may have at most 1,000 current memberships
plus pending trip invites. The `/trips` and member-profile collections also have
a 4 MiB encoded-response ceiling. Trip creation, invite creation, and invite
acceptance compute the projected distinct counts after `BEGIN IMMEDIATE`;
readers use deterministic `LIMIT 1001`, account encoded bytes, and fail closed
without a partial collection. Exact-boundary and concurrent-final-slot tests
cover both dimensions.

Within those aggregate caps, at most 100 pending invites may target one email
digest. Invite creation starts with a short role/cap preflight, closes that
transaction before idempotently adding the email to the reusable human-admission
group shared by the Pages/API Access applications, then starts `BEGIN IMMEDIATE`
and rechecks the inviter's leader role and all three projected counts before
inserting. A race that reaches a cap after the external call may leave app-wide
login temporarily admitted, but it creates no membership and the bounded
desired-state reconciler removes an email with no membership or pending invite.
Service/probe IDs are never group entries. An exact
same-trip repeat does not consume another slot. Invite acceptance locks the
writer and loads pending rows in deterministic order with `LIMIT 101`. More than
100, or a projected aggregate above 1,000, is corruption and fails closed
without a partial acceptance. Otherwise the same transaction inserts any
missing memberships and marks exactly those invitations accepted. An exact
repeat is a no-op, and SQLite's writer reservation orders a concurrent inviter
before or after the acceptance rather than losing its row. Tests cover
99/100/101 pending rows, 999/1,000/1,001 aggregate rows, repeated `/me`,
external-grant/storage failure reconciliation, and inviter-versus-acceptor
races.

### 4.3 Places and candidates

`catalog_places`

- optional provider/cache records keyed by public `id`;
- provider identity and cached facts are never trip authorization.

`trip_places`

- primary key `(trip_id, id)`;
- the complete trip-owned candidate or plan snapshot;
- provider facts occupy typed columns; bounded opening-hours, photos, external
  reference, and guide value objects may use validated JSON;
- immutable snapshots are inserted, not rewritten in place.

`candidates`

- primary key `(trip_id, id)`;
- unique foreign key `(trip_id, place_id)` to the candidate-owned trip place;
- proposer, created time, pitch, tags JSON, status, and revision;
- separate nullable `source_catalog_place_id` and `source_trip_place_id`, with a
  check that at most one is present;
- the trip source uses a composite same-trip foreign key.

A city-name match is never provenance. A manual candidate has neither source
column. Candidate `in_plan` is written only by structural publication.

Candidate collection reads remain limited to 1,000 rows and 4 MiB of encoded
response data. Every mutation that can change the encoded collection -- create,
update, status transition, publication, or content revert -- checks projected
count and bytes after `BEGIN IMMEDIATE`; concurrent writers cannot each consume
the final slot. An exact idempotent no-op remains available even if legacy data
is already over budget. Readers fail closed before returning a partial
collection. Pagination is a separate API-contract change.

Place search is independently bounded to 100 unique results and 4 MiB of
encoded response data. A short preliminary transaction checks direct membership
before any provider cost, then closes. The provider request has a deadline and
may return at most 101 strictly validated results. A later authoritative read
transaction rechecks the human or service grant and direct membership while it
reads saved places in one snapshot with deterministic ordering and `LIMIT 101`;
it never waits on the provider. The service streams both sources through bounded
de-duplication and fails closed with no partial result if the combined unique
count or encoded bytes exceed the limit. Tests cover exact count/byte boundaries,
duplicate IDs, provider timeout or oversize, cross-trip snapshots, revocation
between the preliminary and authoritative checks, and a concurrent saved-place
insert. Pagination or truncation would be a separate API-contract decision.

### 4.4 Versioned plans, days, stops, and legs

`plans`

- primary key `(trip_id, version)`, unique `(trip_id, id)`, and an explicit
  unique parent key `(trip_id, id, version)` for the trip's exact current-plan
  foreign key;
- optional creating proposal, created time, and revision;
- an optional creating proposal uses a deferred same-trip composite foreign key
  `(trip_id, created_from_proposal_id)` to `proposals`;
- a partial unique index on `(trip_id, created_from_proposal_id)` where the
  proposal is non-null prevents one application from minting two versions;
- `version = 1` requires null proposal provenance, while every `version > 1`
  requires it. Imports or system-generated versions would need a separately
  reviewed provenance variant rather than weakening this check;
- all published plan versions remain immutable except for explicitly supported
  current-version content fields stored on child rows. Strict reads require a
  reciprocal one-to-one relationship between every applied proposal and its
  created version; missing, duplicate, non-applied, or cross-trip provenance is
  corruption. Tests cover transaction replay and malformed reciprocal links.

`plan_days`

- primary key `(trip_id, plan_version, id)`;
- composite foreign key `(trip_id, plan_version)` to `plans`;
- unique `(trip_id, plan_version, date)`;
- city, timezone, window fields, and revision.

`stop_identities`

- primary key `(trip_id, id)`;
- a stable stop identity shared by its rows across immutable plan versions.

`plan_stops`

- primary key `(trip_id, plan_version, id)`;
- composite foreign keys `(trip_id, plan_version)` to `plans`,
  `(trip_id, plan_version, day_id)` to `plan_days`, `(trip_id, id)` to
  `stop_identities`, and `(trip_id, place_id)` to `trip_places`;
- unique `(trip_id, plan_version, day_id, seq)`;
- stop kind, schedule fields, notes, optional booking columns, and revision;
- booking `ledgerEntryId` is not duplicated on this row. The API derives that
  read-only field by joining the unique expense whose `linked_stop_id` names
  the stable stop identity.

`route_legs`

- future trip-scoped provider cache keyed by `(trip_id, from_place_id,
to_place_id, mode, departure_bucket)`;
- composite foreign keys `(trip_id, from_place_id)` and
  `(trip_id, to_place_id)` to `trip_places`, so a private place ID cannot cross
  trip scope through the cache;
- provider snapshot time and bounded result fields;
- feasibility views remain derived.

The trip's current pointer names one row in `plans`. Publication inserts a full
new version and advances that pointer in one transaction. It cannot remove a
stop identity linked by the ledger. Unique day dates and stop sequence ordering
are database constraints in addition to domain validation.

Plan-version listings retain at most 1,000 versions and 4 MiB of encoded plan
metadata per trip. Initialization and every structural publication compute the
projected count and bytes after `BEGIN IMMEDIATE`; an exact publication replay
remains available at the ceiling. Readers use version order and `LIMIT 1001` and
fail closed rather than returning partial history. Tests cover exact count/byte
boundaries and concurrent attempts to create the final version.

The current API exposes immutable version metadata as history, not structural
rollback: it cannot load a historical graph or repoint the live pointer. A
future separately reviewed re-adoption command must go through normal
leader/poll governance, copy the selected historical graph into a new
monotonically numbered version, and atomically revalidate current membership,
base revision, candidate state, ledger-linked stops, provenance, idempotency,
and all collection bounds. It must never mutate or simply reactivate old rows.

### 4.5 Content history and safe revert

`content_edits`

- primary key `(trip_id, id)`;
- entity kind, entity ID, allowlisted field, canonical old/new JSON, author,
  source columns, status, creation time, and revision;
- an optional source service is represented by checked source-kind columns and
  a composite foreign key `(author_id, source_service_id)` to the retained
  `(owner_id, id)` mapping. A service ID is never joined without its owner;
- nullable revert actor/time, `revert_edit_id`, and `reverts_edit_id`;
- same-trip deferred self-foreign keys for both provenance directions;
- unique non-null provenance targets and no-self-reference checks make the
  relationship one-to-one;
- only `applied | reverted` rows enter shared content history. Checked columns
  require `reverted_by`, `reverted_at`, and the outgoing `revert_edit_id` to be
  all null exactly while the row is applied, and all present once it is
  reverted. The independent incoming `reverts_edit_id` identifies a
  compensation and may coexist with either status. Strict reads require the two
  directions to be reciprocal. These rules allow the compensation itself to be
  reverted without losing either provenance link;
- `reverts_edit_id` may remain set after a compensation is itself reverted, so
  chains preserve every applied and reverted step rather than overwriting it.

Pending/rejected review material does not enter this table. The current
1,000-row and 4 MiB HTTP safety limits remain until pagination changes the API,
but DynamoDB history-slot rows disappear. `BEGIN IMMEDIATE` serializes appenders;
the repository counts and sizes the bounded graph before inserting.

A revert selects by `(trip_id, edit_id)`, validates the stored event and typed
allowlist, reads the live entity, then updates only when its revision and exact
field value still match. Every direct member may read history; only a current
leader or member may revert. Notices retain the stricter existing rule: a
current editor may revert their own notice, while another author's notice
requires a current leader. The same transaction rechecks the applicable role,
marks the original, and inserts the compensation. A foreign trip ID is
indistinguishable from missing. Reciprocal, chronological, acyclic provenance
is still validated on read; foreign keys alone do not prove chronology.

### 4.6 Structural proposals and polls

`proposals`

- primary key `(trip_id, id)`;
- creator, optional service source, title/rationale, canonical ChangeSet JSON,
  route, status, decision provenance, timestamps, and revision;
- checked source-kind columns pair an optional service ID with the creator, and
  `(creator_id, source_service_id)` is a composite foreign key to the retained
  service mapping;
- decision provenance uses checked relational columns for either a leader user
  or the current deciding poll; the poll link is same-trip and deferred.

`polls`

- primary key `(trip_id, id)`;
- creator, kind, lifecycle times, quorum, multi-select flag, status, resolution,
  decision time, and revision.

`poll_options`

- primary key `(trip_id, poll_id, id)`;
- unique `(trip_id, poll_id, position)` and canonical-label constraints;
- strict reads require positions to be contiguous in deterministic order;
- optional same-trip proposal link only for server-created plan-change polls;
  historical replacement polls may reference the same proposal, while the
  proposal's decision columns identify only the current deciding poll.

`poll_ballots`

- primary key `(trip_id, poll_id, user_id)`;
- ballot time and revision;
- actor is always the authenticated user.

`poll_ballot_options`

- primary key `(trip_id, poll_id, user_id, option_id)`;
- composite foreign keys to the ballot and option.

Opening, voting, closing, proposal routing, and plan publication each use one
write transaction. Role, poll/proposal revision, current plan pointer, quorum
snapshot, candidate status, and source rows are reread after `BEGIN IMMEDIATE`.
No caller can provide a poll/proposal relationship that the server owns.

Proposal and poll collection reads each retain the 1,000-record and 4 MiB
encoded-response ceilings. The proposal ChangeSet remains at 20 operations, and
publication rejects a projected change above 100 created/updated actions or
3 MiB until a separate reviewed contract replaces that safety bound. Count,
ordering, and byte calculations share the authorization snapshot; writers check
the projected result under `BEGIN IMMEDIATE`. Tests cover exact limits,
concurrent final-slot writes, and fail-closed oversized reads.

### 4.7 Discussions

`discussion_threads`

- primary key `(trip_id, id)`;
- anchor kind, nullable anchor ID, canonical `anchor_key`, title, creation and
  activity times, and revision;
- unique `(trip_id, anchor_key)` replaces the hashed anchor claim.

`discussion_comments`

- primary key `(trip_id, thread_id, id)`;
- author, markdown body, creation time, and revision;
- foreign key to the same-trip thread.

`comment_reactions`

- primary key `(trip_id, thread_id, comment_id, emoji, user_id)`;
- foreign keys to comment and stable user.

Thread comment counts are derived. Last activity is stored on the thread and is
advanced with its revision in the same transaction that inserts a comment;
there is no capability metadata row. Anchor existence is validated inside the
write transaction, including the exact current plan for day/stop anchors.
Desired-state reactions use insert/delete of the caller's own primary key and
are idempotent.

Parity permits at most 1,000 threads per trip and 1,000 comments per thread,
with a 4 MiB encoded collection/response budget. Thread/comment writes compute
the projected count and bytes after `BEGIN IMMEDIATE`; reads stop and fail
closed before returning a partial or oversized collection. Exact-boundary and
concurrent-final-slot tests preserve these ceilings. Existing text limits also
remain: 200 title characters, 10,000 comment characters, and 16 reaction
characters.

### 4.8 Ledger and settlements

`expenses`

- primary key `(trip_id, id)`;
- payer, amount/currency/frozen rate, category, note, receipt reference,
  optional stable stop identity, checked split kind, creation time, and revision;
- unique `(trip_id, id, split_kind)` supports a split-kind-safe participant
  foreign key;
- unique `(trip_id, linked_stop_id)` when non-null preserves the singular
  booking pointer;
- composite foreign key `(trip_id, linked_stop_id)` to `stop_identities` when
  the pointer is non-null;
- payer references a stable user, while the write transaction verifies current
  membership.

`expense_split_participants`

- primary key `(trip_id, expense_id, user_id)`;
- a duplicated checked split kind, nullable weight, and nullable exact amount;
- a row check requires neither value for `even`, only a positive weight for
  `shares`, and only a non-negative exact amount for `exact`, matching the
  current Rust contract;
- a composite foreign key `(trip_id, expense_id, split_kind)` to the expense
  prevents child and parent split kinds from disagreeing; the user foreign key
  preserves historical identity.

`settlements`

- primary key `(trip_id, id)`;
- distinct sender/recipient, amount, settled time, and creation revision;
- sender and recipient each have stable-user foreign keys.

`ledger_events`

- primary key `(trip_id, id)`;
- checked action, actor, time, checked entity kind, entity ID, canonical
  before/after JSON, and nullable predecessor;
- the typed entity columns must agree with the IDs and variants in before/after
  snapshots;
- a same-trip predecessor foreign key and a unique non-null predecessor permit
  at most one successor, while a partial unique index on `trip_id` where the
  predecessor is null permits at most one root;
- strict reads require exactly one root and one head for a non-empty ledger,
  follow the head to the root, and require the visited count to equal the
  derived `COUNT(*)`. They also validate chronological order and every
  before/after transition, so disconnected cycles or branches fail closed;
- the latest reconstructed state for each entity must equal its live expense or
  settlement row, and every live row must have the matching create history.
  Retained idempotency claims must recompute to the same canonical create
  request, event, and immutable result. Missing creates, revision drift, forged
  results, or provenance mismatches fail closed.

Parity keeps the current per-trip ceilings: at most 1,000 expenses, 1,000
settlements, 1,000 distinct people in derived balances, 4,000 ledger events,
and 4,000 retained ledger idempotency claims, with a 4 MiB aggregate
read/serialized-response budget. Writes derive the exact counts and projected
bytes after `BEGIN IMMEDIATE`; idempotent replays remain available at the
boundary. Reads fail closed rather than materializing an unbounded graph.
Pagination or streaming may replace these limits only through a separate
API-contract change.

Expense stop claims, ledger metadata rows, and the duplicated booking-side
ledger pointer disappear. The unique expense link and stable `stop_identities`
are the single stored relationship; plan reads derive `ledgerEntryId` with a
join. A write still verifies that the stop exists in the exact current plan,
has a booking, and has the expected revision before changing the expense link.
Booking edits and structural publication read that same relationship in their
write transaction, preserve it in history output, and reject removal of a
linked booking or stop. Historical member references remain readable after
membership removal.

### 4.9 Notices and checklists

`notices`

- primary key `(trip_id, id)`;
- author, category, title/body/source, pin/lifecycle, creation time, revision,
  and checked `whole_group | explicit` audience mode.

`notice_audience`

- primary key `(trip_id, notice_id, user_id)`;
- an explicit audience has one row per selected user; a whole-group notice has
  none. The stored mode prevents a missing or partially deleted explicit
  audience from silently widening access, and strict reads reject an explicit
  audience with zero rows.

`notice_checklist_items`

- primary key `(trip_id, notice_id, id)`;
- text, optional due date, checked mode, and stable position;
- unique `(trip_id, notice_id, position)`, with strict reads requiring the
  bounded positions to form the canonical contiguous order;
- unique `(trip_id, notice_id, id, mode)` supports mode-safe completion foreign
  keys.

`notice_each_completions`

- primary key `(trip_id, notice_id, item_id, user_id)`;
- a checked constant `item_mode = 'each'`, completion time, and a composite
  foreign key `(trip_id, notice_id, item_id, item_mode)` to the checklist item.

`notice_group_completions`

- primary key `(trip_id, notice_id, item_id)`;
- a checked constant `item_mode = 'group'`, completing user and time, with the
  same mode-safe composite foreign key. The key structurally permits one group
  stamp without a cross-table or parent-dependent partial index.

Notice metadata/count rows disappear. Counts and audience progress are SQL
queries. An audience update validates current memberships and deletes excluded
completion rows in the same transaction. Removing a trip membership does not
silently erase historical notice state; the documented authorized audience
update remains the cleanup path.

Parity permits at most 1,000 notices per trip, 100 checklist items and 90
explicit audience users per notice, and 1,000 completion users per checklist
item. The complete encoded notice collection/response remains capped at 4 MiB.
After `BEGIN IMMEDIATE`, writers check projected normalized row counts and
encoded output before inserting; readers fail closed before returning partial
or oversized state. Tests cover every exact boundary, duplicate/gapped
checklist positions, and concurrent writers contending for the final slot.

### 4.10 Idempotency claims

`ledger_idempotency_claims`

- primary key `(trip_id, key_digest)`;
- actor, endpoint, canonical request hash, strict immutable creation-result
  JSON, result ID/kind, and creation time;
- retained and bounded to 4,000 rows per trip, matching current replay history.

`notice_idempotency_claims`

- primary key `(trip_id, actor_id, key_digest)`;
- endpoint, canonical request hash, compact server-owned notice/item IDs,
  creation time, and exact expiry;
- application-expired after 24 hours and bounded to 32 live claims per
  actor/trip.

The tables stay capability-owned because their actor scope, retention, and
replay values intentionally differ. A claim is inserted in the same transaction
as its result. Ledger replay returns the immutable original creation result;
notice replay resolves the compact IDs through current trip-scoped state.
Different actor, endpoint, request, or result conflicts according to that
capability's existing contract. Notice expiry and bounds use application time
under the same writer lock. For a notice command, `BEGIN IMMEDIATE` first loads
the actor's exact key: a live exact endpoint/request replays; a live mismatch
conflicts; and an expired same-key row is replaced in the result transaction.
For a new key below 32 physical claims, the result and claim insert together.
At 32, the transaction deterministically deletes the oldest expired claim
(expiry then key digest) while inserting the replacement, or rejects when all
32 are live. Physical cleanup is never replay authority. Tests cover same-key
expiry, conflicting reuse, deterministic eviction, all-live capacity, and
concurrent boundary writers. Rust may share hashing and canonicalization
helpers, but not a generic repository that erases those semantics.

### 4.11 Service identities and quota

`service_identities`

- primary key `(owner_id, id)`;
- owner foreign key to stable `users`;
- globally unique `client_id_digest`, short display hint, name, expiry,
  revocation, created/last-used times, and revision;
- the row is tombstoned, never deleted, so the digest cannot be rebound;
- no raw client ID or client secret column exists.

`service_identity_scopes`

- primary key `(owner_id, service_id, scope)` with `read | propose` check and a
  composite foreign key `(owner_id, service_id)` to the retained mapping.

`service_identity_trips`

- primary key `(owner_id, service_id, trip_id)` with foreign keys to mapping and
  trip.

`service_usage_buckets`

- primary key `(owner_id, service_id, bucket_start)`;
- request count constrained to 1-300, last-used time, and exact expiry;
- composite foreign key `(owner_id, service_id)` to the retained mapping.

The unique digest on the retained mapping replaces reciprocal mapping/claim
rows. Authentication starts a write transaction, resolves the exact active
mapping by digest, checks expiry/revocation/scopes, and atomically inserts or
increments the current bucket only while its count is below 300. Revocation
uses the same writer lock and therefore cannot race quota consumption. Each
trip operation still checks the owner's direct membership; the mapping is only
an additional restriction. Registration counts retained mappings and selected
trips under the writer lock, enforcing the existing limits of 50 mappings per
owner and 20 trips per mapping. It rechecks current direct membership for every
trip, including editor authority for `propose`.

Usage retention is physical housekeeping, never quota authority. An unexpired
mapping normally has at most the current bucket plus 48 prior hourly buckets;
each row's expiry must equal its bucket close plus 48 hours. Before consuming the
current bucket, the same auth writer transaction rejects malformed or
future-dated rows and deterministically deletes at most 128 valid expired rows
for that mapping by primary-key order. A bounded startup/maintenance catch-up
continues global deletion in fixed 500-row transactions with a durable cursor
and runtime limit; interruption resumes later and does not reinterpret expiry.
Tombstoned mappings remain as FK parents while their expired usage rows are
eventually removed. Tests cover exact expiry, multi-year downtime/backlog,
interrupted catch-up, malformed/future rows, and concurrent auth/revocation.

### 4.12 Future review queue

`review_items` is reserved for the separately reviewed owner-airlock feature.
It will be keyed `(owner_id, id)`, include trip context and a typed bounded
payload, keep pending/approved/rejected provenance, and use
`(owner_id, source_service_id)` as a composite foreign key to the retained
mapping. It is not created merely to make current `propose` service scope
writable; service mutations remain fail-closed until that feature exists.

## 5. Authorization and transaction recipes

### Reads

A private read begins a transaction, performs:

```sql
SELECT role
FROM trip_memberships
WHERE trip_id = ?1 AND user_id = ?2;
```

and reads the trip-owned rows before committing that same snapshot. Listing a
user's trips may start from the `(user_id, trip_id)` index, but opening each trip
still uses the direct membership key. The list uses deterministic order,
`LIMIT 1001`, and the 4 MiB response budget described above. There is no
eventually consistent index; the direct-read rule remains because it prevents
authorization from drifting into navigation code.

For a service principal, that same read transaction also reloads the retained
mapping by owner/service ID and requires it to remain active, unexpired, scoped
for `read`, and allowlisted for the route trip before checking the owner's
membership. A future review-queue write repeats the active mapping, `propose`
scope, trip allowlist, and owner membership checks after `BEGIN IMMEDIATE`.

The current API/application path reduces service access to an owner `UserId`
before repository calls, so it cannot implement that promise unchanged. Before
SQLite runtime, `AuthenticatedPrincipal::require_trip_read`, application
services, and every trip capability port must carry a typed authorization
context: either a human user or a service owner plus retained service ID. The
SQLite repository consumes that context inside its transaction; it never trusts
a precomputed boolean grant. Cross-capability contract tests revoke the mapping,
remove a trip allowlist/scope, or change owner membership between authentication
quota consumption and repository access and require the protected read/write to
fail closed.

The current `TripRepo::get_members` shape delegates profile loads to a separate
`UserRepo`, which would open another snapshot. Before SQLite becomes runtime,
that port must change so the trip adapter joins stable user profiles in the same
read transaction, or it must accept a transaction-scoped unit of work. A
concurrent membership/profile test proves the returned authorization and rows
come from one snapshot; repository composition may not silently open a second
connection.

### Writes

Every trip-owned mutation starts `BEGIN IMMEDIATE` before it reads
authorization. This acquires SQLite's writer reservation, so another
transaction cannot revoke or change the role after the check and commit a
conflicting write first. The transaction then:

1. reads the exact direct membership and required role;
2. reads and strictly decodes the current aggregate;
3. validates caller-owned IDs and provider inputs;
4. compares revisions and exact stale-sensitive values;
5. writes domain state, audit, and idempotency rows; and
6. commits once.

`SELECT` followed later by a separate write transaction is not sufficient for
authorization. A Rust preflight may improve errors, but the transaction repeats
the authoritative checks.

Commands without a pre-existing trip membership have equally explicit writer
recipes:

- first login and `/me` profile writes derive identity from the verified human
  assertion, lock the canonical email claim, and atomically resolve or create
  only that stable user/claim pair;
- trip creation derives the creator from the verified human principal and
  inserts the trip plus their leader membership in one transaction;
- invite acceptance locks invitations for the verified user's canonical email
  digest, creates only those memberships, and marks the same invitations
  accepted atomically; and
- owner-scoped service-identity management keys every row by the verified human
  owner. Registration additionally rechecks the selected trip memberships and
  editor role for `propose`; revocation can tombstone only that owner's mapping.

These recipes do not create an alternate path to existing trip content. Any
command that mutates a pre-existing trip aggregate still performs the direct
membership/role check in its writer transaction; a read-only command performs
the check and protected reads in the same read snapshot.

No database transaction waits on a provider or other network call. A bounded,
strictly validated provider result such as an FX rate is fetched first; after
`BEGIN IMMEDIATE`, the repository rereads membership and all authoritative
database context before deciding whether that result may be committed.

### Concurrent and ambiguous outcomes

SQLite busy/locked outcomes receive bounded complete-operation retries only
when the driver proves the transaction rolled back or never reached commit. A
retry reopens the transaction and rereads membership and state. After a failure
during or after `COMMIT`, only an endpoint with a caller-bound idempotency claim
may resolve or replay the exact command from that claim. A generated audit row
does not identify an otherwise tokenless request. Other commands return an
indeterminate outcome, perform no blind automatic retry, and require a fresh
aggregate/history read plus an explicit revision-checked reconciliation. Tests
distinguish definitely pre-commit `BUSY` from a possibly committed connection
failure and never assume that a timeout committed.

## 6. Migrations

Versioned SQL migrations live in `backend/crates/adapters/migrations/` and are
embedded/checksummed by the selected SQL library. Rules:

- a migration is immutable after merge; corrections are new migrations;
- CI applies every migration to an empty temporary file and upgrades fixtures
  from each supported prior schema;
- ordinary migrations keep foreign-key enforcement enabled and end with
  `PRAGMA foreign_key_check` plus an integrity check;
- a referenced-table rebuild follows SQLite's documented outage sequence on a
  dedicated connection: while outside any transaction, disable foreign keys
  only when required and verify the setting; `BEGIN IMMEDIATE`; create, copy,
  verify, drop, rename, and recreate indexes/triggers; require
  `foreign_key_check` to return no rows; then commit, re-enable foreign keys
  outside the transaction, verify they are on, and require a final zero-row
  `foreign_key_check` plus `integrity_check = 'ok'`. Every error path rolls back
  where possible, re-enables enforcement in a finally/fail-closed step, and
  prevents app startup. Changing `PRAGMA foreign_keys` inside a transaction is
  forbidden because SQLite treats it as a no-op;
- normal app startup never auto-migrates; and
- every image accepts one exact migration checksum and schema version. A deploy
  clones the quiescent backup into a new generation and migrates only that
  generation. Failure before ingress opens reselects the intact prior
  generation and its manifest-selected image; if that generation is invalid,
  recovery rebuilds it from the verified backup. Old code is never asked to
  infer compatibility with a newer schema.

Migration and restore-validation commands run as non-root, networkless
maintenance containers with read-only roots, no capabilities/extra privileges,
bounded tmpfs/resources, no secrets or host credentials, and only the exact
proposed/staged generation mount. The host, not these commands, performs image
pulls and object-storage traffic. Tests deny IMDS/internet access and writes to
current or unrelated generations.

The first migration creates the complete schema needed by the first SQLite
capability slice. Later capability PRs may add tables before runtime cutover.
Unused tables are acceptable during this pre-production migration; fake or
partial runtime fallbacks are not.

## 7. Backup, restore, and file handling

The data volume stores immutable-named generation roots such as
`generations/<generation-id>/`. A root-owned manifest at that root binds
`db/itinera.db` and its schema to the exact app image digest and SQLite source
ID; a root-owned `current` symlink selects the active generation, and the host
launcher derives the image from its manifest. Switching the symlink therefore
selects DB and compatible image together. The application container receives
only the resolved `current/db` child as its writable persistent mount, so it
cannot unlink or change the parent manifest. Its `.db`, `-wal`, and `-shm` files
are one unit while live, and the symlink never changes while a database process
is running. A deploy never migrates an existing generation in place: while the
app is stopped it creates a new generation from the verified quiescent backup,
migrates and validates it, atomically writes and `fsync`s a new host-only
manifest, and only then switches `current`. The deploy journal records manifest
installation and selection so a crash cannot pair migrated bytes with the old
image. Shell `copy`, `cp`, EBS file-level snapshots, and container image layers
are not application backups.

Daily backups use a bounded one-shot command from the source generation's
manifest-selected current image/SQLite build to call SQLite's Online Backup API
and create a standalone snapshot while the API may remain online. A pre-deploy
backup uses that same current image only after the app has stopped and closed
SQLite, even if the proposed image is already present, so no acknowledged write
can land after the rollback point and no new binary can relabel old bytes. The
command does not construct domain repositories, opens the source with SQLite
read-only mode, and runs as the same fixed UID in the networkless maintenance
profile. It mounts only the source directory read-only and one journal-owned
destination writable. Tests prove attempted DML against its source connection,
writes outside the stage, IMDS, and internet access are rejected.
It emits the snapshot only after `PRAGMA integrity_check` returns `ok`,
`PRAGMA foreign_key_check` returns no rows, and it computes a checksum; the host
uploads that snapshot and manifest to S3 using credentials that never enter the
container. Before starting, it requires enough free space for the database,
current WAL, snapshot, and a 1 GiB data-volume safety margin; a duration limit
prevents a stuck online reader from growing the WAL indefinitely.

Each job uses one journal-owned unique staging directory under the maintenance
lock. A daily job removes that staged snapshot/directory and `fsync`s the staging
parent only after S3 confirms both objects and their version/checksum. A
pre-deploy job keeps the same journal-owned stage until it has also installed
the standalone snapshot into the proposed new generation, then removes and
`fsync`s only that consumed stage. Every failure path removes its incomplete
stage before releasing the lock and alerts; boot/timer startup removes only the
exact stale stage named by its durable journal. At most one size-bounded stage
can exist, so artifacts cannot accumulate on the data volume.

Deploy uses a larger peak formula than the daily job. Before stopping ingress,
and again after the snapshot has an exact size but before new-generation
installation/migration, it reserves the complete current database/WAL/SHM,
every non-current generation not safely removable, the standalone snapshot, a
complete new generation, the migration's CI-measured and manifest-declared
maximum growth/rebuild/WAL scratch, operation metadata, and the 1 GiB margin.
Only a non-current, non-fallback generation with a verified off-instance backup
may be removed under the lock. A failed proof cleans only the incomplete stage,
keeps/reselects the current generation, and requires EBS expansion. Boundary and
migration-growth fault tests exercise both preflight checks.

The versioned backup manifest records backup format/source-generation ID, UTC
creation time, database byte size and SHA-256, and the source generation
manifest's exact migration version/checksum, app image digest, and
`sqlite_version()`/`sqlite_source_id()`. Only the cloned and migrated new
generation receives the proposed image/source identity. The snapshot file and
its staging directory are `fsync`ed before upload. ECR retention keeps the current
and validated fallback generation digests plus every digest referenced by the
retained backup window. Immutable release tags may act as garbage-collection
roots, but the host always selects an image by digest.

Restore always targets a new generation:

1. take the maintenance lock, fetch and verify the small backup manifest, and
   `fsync` a restore phase journal containing the old target,
   `ingress_closed=false`, and the backup/image identities. While the current
   app still serves, prove the complete root/EBS peak formulas, perform only
   eligible scoped cleanup, and pull any absent compatible image;
2. stop Tunnel ingress and the application, record `ingress_closed=true`, then
   download the snapshot into a new generation. Repeat both space proofs using
   actual allocated bytes and verify size/hash, schema identity, SQLite source
   identity, integrity, and zero foreign-key violations;
3. set the fixed UID/GID and mode, `fsync` the snapshot and generation
   directory, create a temporary `current` symlink, atomically rename it over
   the old symlink, and `fsync` the generation-parent directory;
4. retain the old generation unchanged as quarantine, so old WAL sidecars can
   never be replayed beside the restored file and a crash leaves `current`
   durably naming either the old or new complete generation;
5. record promotion, open the selected generation with the exact image digest
   recorded by the manifest, and repeat integrity/foreign-key, exact schema, and
   SQLite-source checks without mutating the restored generation; and
6. start the app, require database readiness plus representative reads on the
   non-Tunnel port 3001, then `fsync` a `ready_to_open` phase, start Tunnel
   ingress, and require the assertion-protected no-data `/healthz` through the
   public API hostname before recording completion and clearing the journal.

Restore never upgrades the snapshot. After service is recovered on the
manifest's exact image/schema, a later normal deployment may take a fresh
quiescent backup and apply reviewed migrations. This keeps source hash/size and
schema validation meaningful and avoids guessing that a different image is
compatible.

Restore's EBS proof includes the complete current database/WAL/SHM, every
retained non-current generation not safely removable, the complete restored
generation at manifest size plus filesystem allowance, operation metadata, and
the 1 GiB margin. Before outage it safely removes enough excess non-current
generations to keep no more than two after the current becomes quarantine;
every removed generation must have a verified off-instance backup and be
neither current nor an operation fallback. Its journal removes only an
incomplete stage before promotion. On image-open/readiness failure or reboot
before ingress opens, recovery resumes the recorded phase or atomically repoints
and `fsync`s `current` to the intact prior generation when it was recorded as a
validated fallback; that generation's manifest selects its old image digest
too. Once `ready_to_open` is durable, recovery never repoints older data and
only finishes Tunnel/external health, because ingress may already have accepted
writes. If no valid fallback exists, all services stay closed for another
explicit backup selection. Failure to prove either volume's space or a safe
cleanup target stops recovery before mutation for expansion/new-host restore
rather than risking a full filesystem.

Boot recovery validates the journal and `current` target before app startup.
If its manifest-selected image is absent, it applies the same root-space and
protected-digest policy before pulling; failure leaves app/Tunnel closed for
disposable-host replacement rather than consuming SSM headroom.
Fault-injection tests crash after every write, `fsync`, symlink rename,
image-open phase, post-promotion readiness check, `ready_to_open`, and Tunnel
start. They prove recovery always finds or selects a complete old/new
database-and-image generation, never an unpaired database/WAL set or a missing
active name, and never rolls accepted writes back after ingress may have opened.

Daily backup, deploy, migration, restore, host patch, and reboot units share one
host-maintenance lock and explicit systemd conflict/ordering rules. Tests race
timer activation against deployment and restore; no second maintenance command
may open or replace the database until the lock holder exits. Timers stay
enabled and fail/retry lock acquisition rather than being disabled across a
crash window; fault tests kill the deploy before its first journal record.

Deploy runs as a host systemd one-shot with an `fsync`ed phase journal on the
data volume. The record binds old/new generation and image identities, backup
object/hash/schema, manifest installation, `current` selection, and
ingress/migration state. Boot recovery gates both app and `cloudflared`, then
resumes or reselects the intact prior database-and-image generation under the
same lock after SSM loss, process kill, or reboot; an invalid prior generation
is reconstructed only from the verified backup. Fault injection covers every
recorded phase, including `ready_to_open` and Tunnel startup.

The complete operational, retention, RPO, and RTO contract is in the ADR.

## 8. Testing gate

Repository tests do not mock SQL calls. Each test receives a real temporary
file, applies migrations, and opens the same configured pool as production.
Each capability contract suite runs against SQLite as that capability lands,
with provider-specific mechanical assertions kept separate from the shared
repository contract. The archived DynamoDB suite is not an active comparison
target.

Every capability covers:

- cross-trip isolation and foreign child IDs;
- viewer/member/leader/service permissions;
- role or membership change racing a mutation;
- stale revisions and exact-value checks;
- malformed persisted JSON and invalid enum/timestamp data;
- raw wrong-type/NULL-key inserts, revision conversion/overflow, and foreign-key,
  unique, and check-constraint failures;
- concurrent writers using separate connections and barriers;
- rollback after failure at each multi-row mutation boundary;
- idempotent replay and conflicting key reuse; and
- migrations from an empty file and each retained prior fixture.

History additionally tests corrupt/cyclic/one-sided provenance and concurrent
revert. Polls test ballot/close serialization and exact collection bounds.
Ledger tests derived booking links, unique expense ownership, every row/byte
ceiling, and immutable audit chains, including disjoint roots, cycles,
orphaned/omitted events, and wrong entity IDs. Notices test
audience/completion cleanup, checklist ordering, claim eviction, and all
normalized-row/response limits. Service identities test digest uniqueness,
expiry, revoke/quota races, raw zero-count rejection, and the exact 300-request
boundary.

HTTP integration tests may extend test-target fakes for router coverage while
the ports migrate, but the runtime binary never gains an in-memory persistence
fallback.

## 9. Runtime restoration gate

There is no transitional persistence runtime, per-request adapter switch, or
dual write. A production-shaped API binary returns only when all repository
contract suites pass for SQLite and:

- startup accepts a required absolute SQLite database path and no Dynamo table;
- the readiness-only listener proves schema compatibility and a database read,
  while the separate Tunnel-origin health probe proves the Access/JWT path;
- default and all-feature test suites pass with the SQLite runtime;
- container, backup, restore, and deploy rollback drills pass; and
- the private environment is still absent.

The archived DynamoDB adapter, AWS SDK dependencies, and Lambda entrypoint are
not reintroduced. The old infrastructure and edge Worker remain frozen until
their replacement slices and must not be applied. If live or irreplaceable data
appears before runtime restoration, work stops for a separate migration ADR.

## 10. References

- [SQLite foreign keys](https://www.sqlite.org/foreignkeys.html)
- [SQLite transactions](https://www.sqlite.org/lang_transaction.html)
- [SQLite write-ahead logging](https://www.sqlite.org/wal.html)
- [SQLite WAL-reset bug and fixed versions](https://www.sqlite.org/wal.html#the_wal_reset_bug)
- [SQLite Online Backup API](https://www.sqlite.org/backup.html)
- [SQLite PRAGMA reference](https://www.sqlite.org/pragma.html)
