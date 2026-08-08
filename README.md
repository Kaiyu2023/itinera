# Itinera

_Latin: the plural of **iter** — journeys, roads._

Itinera is a collaborative trip-planning app I wrote for planning trips with my
friends: one shared itinerary on a map, candidate places we pitch and vote on,
structural plan changes that go through a poll or a leader's approval instead of
silently rewriting the plan, a shared expense ledger, and a "before you go" board
for the boring-but-important prep. It's built for our trips first — but if you
find it interesting or useful, you're free to use it, fork it, or take pieces of
it (MIT licensed).

## Documents

- [Design document](docs/DESIGN.md) — architecture, data model, and product design.
- [Security guide](docs/SECURITY.md) — how private trip data, shared plans, and
  the owner's cloud bill are protected.
- [Architecture decision](docs/adr/0001-single-node-sqlite.md) — why the target
  is one EC2 host, Cloudflare Tunnel, and SQLite.
- [SQLite design](docs/SQLITE.md) — target schema, transactions, migrations,
  backup, restore, and repository test contract.
- [Frozen AWS infrastructure module](infra/README.md) — obsolete
  Lambda/DynamoDB resources retained only until their reviewed replacement; do
  not deploy them.
- [API contract](docs/openapi.yaml) — the single source of truth for the backend API.

## Tech at a glance

| Layer       | Target choice                                                        |
| ----------- | -------------------------------------------------------------------- |
| Backend     | Rust (axum), one ARM64 container managed by systemd on EC2          |
| Frontend    | TypeScript + React (Vite), hosted on Cloudflare Pages                |
| Database    | SQLite in WAL mode on a retained encrypted EBS data volume           |
| Maps        | Google Maps Platform (Essentials tier) behind provider traits        |
| Auth/ingress | Cloudflare Access + outbound Cloudflare Tunnel; no public host port |
| Infra       | Terraform child module; private root deploys through AWS OIDC        |

**Design rule #1:** every external service sits behind an interface (Rust trait /
TypeScript interface) so providers can be swapped without touching callers.

**Workflow:** frontend first (Claude), built against a `MockApiClient` and fixture
data; once the frontend is frozen, its API contract (`docs/openapi.yaml`) becomes
the spec for the Rust backend (Kaiyu), and `HttpApiClient` swaps in.

## Status

Phase A (complete): the full frontend against the in-memory mock, with realistic
fixture data and a Playwright suite covering both desktop and mobile viewports.
Phase B (in progress): authentication and the complete trip domain and HTTP
contract are implemented. Their former Lambda/DynamoDB persistence backend has
been archived before the SQLite rewrite. The Rust API contract covers trip and
member operations, candidate-owned place snapshots, and versioned plan day/stop
operations through trip-scoped repository ports; Cloudflare invite grants and
the public place catalog deliberately fail closed until their provider adapters
are added.
The first Phase B product-domain slice is also specified: every current member
may read field-level content history, while leaders and members can perform an
allowlisted, atomic safe revert by server-issued edit id. A revert preserves the
original event, records actor/time provenance, and appends a compensating edit.
Shared history contains applied/reverted content only and fails closed at the
documented row and byte budgets until cursor pagination lands.
Human structural proposals are implemented. Every direct member
may inspect them; leaders and members may submit a bounded ChangeSet; only a
leader may approve or reject. Leader-owned submissions apply immediately, while
member submissions wait for a leader. Application creates an immutable next plan
version in one stale-safe repository transaction. Polls are implemented as a
separate capability: every direct member may read them, leaders and members may
create decision polls and vote, and only leaders may close them. Quorum excludes
viewers and is frozen when the poll is created. Proposal-routed plan-change
polls are server-linked atomically and apply through the same stale-safe plan
publication boundary; caller JSON can never attach an arbitrary proposal.
Discussions are also implemented as their own repository capability. Every direct
member may read them; leaders and members may create one atomic thread per
trip-owned anchor, comment, and set their own reaction state. Current-plan
anchors are stale-safe, writes recheck role transactionally, and viewers remain
read-only.
The shared ledger is implemented as a separate repository capability. Every direct
member may read derived balances; leaders and members may add or correct
expenses and record settlements, while viewers remain read-only. Exchange rates
are fetched and frozen server-side, payer/split/settlement membership is
rechecked in each write transaction, stop links are reconciled atomically, and
ledger-specific audit events preserve correction and deletion provenance.
Expense and settlement creation require an operation key, so an exact retry
returns the original server-owned result while reuse by another actor or for a
different request conflicts. Audit predecessors and recomputed request hashes
make stored provenance independently checkable. Booking-side ledger pointers
are output-only: plan edits and history reverts preserve them, while structural
publication validates and snapshot-guards the full pointer/claim/expense graph.
Notices and checklists have their own bounded repository capability.
Every direct member may read and acknowledge applicable checklist items;
leaders and members may create notices, while management and safe revert retain
the stricter current-author-or-leader rule. Notice edits append field-level
content history atomically, explicit audiences are rechecked as direct members,
and callers can never select another user's completion state. Notice creation
and checklist toggles require actor-scoped, 24-hour idempotency keys; ordinary
notice reads never scan those bounded claim partitions. Audience changes and
reverts remove now-excluded completion stamps on the server, and every content
audit writer reserves the same global history row/byte budget.
The Cloudflare Access service-identity domain and HTTP contract are implemented
without a custom bearer-token path. Human owners register an externally created
service-token client ID in Cloudflare's canonical 32-lowercase-hex + `.access`
form against
explicit trips and `read`/`propose` scopes; persistence stores only a digest and
short hint, transactionally rechecks membership, and enforces a
300-request UTC-hour limit. Service assertions never auto-provision people.
Scoped reads must still pass through each trip repository's transactional
membership check, while every direct mutation, vote, approval, and
administrative route remains human-only until the owner review queue lands.
Revocation atomically tombstones the global claim and owner mapping.
The deployment and persistence design has now moved to a simpler single-node
target: Cloudflare Tunnel reaches one systemd-managed EC2 container, which uses
SQLite on a retained encrypted EBS data volume. Because no private environment
or live production data exists, development now uses a clean break: the former
DynamoDB/Lambda backend is preserved on `codex/dynamodb-archive` but removed
from the active code before the SQLite capability ports are built. There is no
dual-write or live data-conversion phase. The remaining plan completes this
migration, then integrations and frontend cutover, and only later creates the
private production environment; see
[the ordered implementation plan](docs/DESIGN.md#12-implementation-plan).

There is deliberately no runnable persistence-backed API binary during the
clean-break rewrite. The API library and router contracts remain testable with
test-target-only fakes, while each SQLite capability is exercised against real
temporary files. Runtime startup returns only after the required SQLite
repositories and readiness contract are complete.

Trip, membership, invite, candidate/place, and plan/day/stop data now cross the
SQLite boundary through validated domain construction. Canonical currencies use
a private `CurrencyCode` newtype; `TripMember`, `DateRange`, and `SoftBudget`
validate their own values; and converting `TripData` into `Trip` checks
aggregate-wide invariants. Candidate and plan codecs decode query-shaped SQL
rows directly into the existing domain values and run the same canonical
validators used by application services. SQLite remains responsible for
relational constraints, transaction-time authorization, revisions, corruption
mapping, and resource bounds rather than inventing persistence-only domain
models.

The first four pre-runtime SQLite capability slices are implemented: `SqliteDb`
owns a bounded, version-checked pool; versioned migrations and separate
repositories persist users, trips, memberships, invitations, candidate-owned
place snapshots, versioned plans, field-level content history, structural
proposals, and normalized polls/ballots. Trip status,
candidate, current-day, and current-stop edits append typed audit rows in the
same `BEGIN IMMEDIATE` transaction as their exact-revision entity update.
`SqliteContentHistoryRepo` validates the complete reciprocal history graph in
one authorized snapshot and performs stale-safe reverts by restoring only the
stored old value and appending a compensation. `SqliteProposalRepo` and
`SqlitePollRepo` authorize in their own snapshots, serialize every governance
mutation with `BEGIN IMMEDIATE`, publish immutable next-plan versions, and
require reciprocal proposal/poll/plan/candidate-audit provenance. Each plan
transition is independently replayable from its canonical ChangeSet, generated
identities, structural-audit manifest, and base/result hashes. Real
temporary-file tests cover retained upgrades and strict schema behavior,
transaction-time authorization, exact row/byte/action ceilings, corruption,
rollback, revision exhaustion, and concurrent writers, ballots, closes, and
reverts. Service-authored rows, notice edits, and ledger-linked booking history
remain unreadable until their owning SQLite capabilities can validate the
missing reciprocal records. This does not select SQLite in `AppState`,
introduce dual writes, or restore a runnable API binary. Each remaining
capability must pass the same gate before runtime cutover; fast router tests may
continue to use test-target-only fakes that cannot enter the application
binary.

The pre-runtime authorization prerequisite is also complete. Application
services and every trip capability port now carry either a human principal or
a service owner together with the retained service ID. Implemented SQLite trip
reads and writes recheck human membership inside their own transaction and fail
closed before data access for services until the SQLite service-identity
capability can recheck the mapping, scope, trip allowlist, and owner membership
there. Trip creation and invitation acceptance also verify, after acquiring the
writer transaction, that the context is the matching human creator or invitee.
Member/profile reads are one SQLite join and snapshot; the trip port can no
longer compose them through a second user-repository connection.

## Development

```sh
cd frontend
npm install        # also wires up the pre-commit hook
npm run dev        # dev server
npm run test:e2e   # Playwright suite (desktop + mobile projects)
npm run lint       # oxlint
npm run typecheck  # tsc
npm run format     # prettier
npm run openapi:lint  # validate docs/openapi.yaml as OpenAPI 3.1
npm run test:contract # check ApiClient, schemas, routes, and Rust route coverage
```

The pre-commit hook runs formatting, lint, type, and API-contract checks; CI enforces the same
static checks, plus the full e2e suite, backend tests, and mocked Terraform
module tests, on every PR.

Terraform is not required for normal application development. Contributors who
change `infra/` can validate it without AWS credentials or a remote backend:

```sh
cd infra
terraform fmt -check -recursive
terraform init -backend=false
terraform validate
terraform test
```

Real plans, state, identifiers, and deployments live only in the private
deployment repository; see the [module guide](infra/README.md).

## Contributing

This is a personal project shared with friends; collaborators are added by
invitation. Issues and forks are welcome, but expect PRs from outside the group
to move slowly — it's a hobby, not a product.

## Secrets

No credentials of any kind live in this repository — not in code, config,
fixtures, or CI files. AWS deployment uses short-lived GitHub OIDC credentials;
runtime secrets are installed from the private deployment workflow into their
managed service and never passed through this public Terraform module. If you
fork this to deploy your own, keep the same boundary.

## License & assets

Code is [MIT](LICENSE). The place photos are from Wikimedia Commons under their
individual licenses — see
[`frontend/public/ATTRIBUTIONS.md`](frontend/public/ATTRIBUTIONS.md).
