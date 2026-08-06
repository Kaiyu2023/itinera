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
- [DynamoDB design](docs/DYNAMODB.md) — physical keys, access patterns,
  consistency rules, capacity, recovery, and least-privilege IAM.
- [AWS infrastructure module](infra/README.md) — public resources, safe
  defaults, private deployment boundary, and local validation.
- [API contract](docs/openapi.yaml) — the single source of truth for the backend API.

## Tech at a glance

| Layer    | Choice                                                        |
| -------- | ------------------------------------------------------------- |
| Backend  | Rust (axum) on AWS Lambda                                     |
| Frontend | TypeScript + React (Vite), hosted on Cloudflare Pages         |
| Database | Amazon DynamoDB (one table, provisioned free tier)            |
| Maps     | Google Maps Platform (Essentials tier) behind provider traits |
| Auth     | Cloudflare Access one-time PIN login (free ≤ 50 users)        |
| Edge     | TypeScript Worker → JavaScript proof gate and Lambda OAC      |
| Infra    | Terraform child module; private root deploys through AWS OIDC |

**Design rule #1:** every external service sits behind an interface (Rust trait /
TypeScript interface) so providers can be swapped without touching callers.

**Workflow:** frontend first (Claude), built against a `MockApiClient` and fixture
data; once the frontend is frozen, its API contract (`docs/openapi.yaml`) becomes
the spec for the Rust backend (Kaiyu), and `HttpApiClient` swaps in.

## Status

Phase A (complete): the full frontend against the in-memory mock, with realistic
fixture data and a Playwright suite covering both desktop and mobile viewports.
Phase B (in progress): authentication, user persistence, the protected AWS
origin, and the complete trip core are implemented. The Rust API now serves
trip and member operations, candidate-owned place snapshots, and versioned plan
shells through trip-scoped repositories; Cloudflare invite grants and the public
place catalog deliberately fail closed until their provider adapters are added.
The first Phase B product-domain slice is also live: every current member may
read field-level content history, while leaders and members can perform an
allowlisted, atomic safe revert by server-issued edit id. A revert preserves the
original event, records actor/time provenance, and appends a compensating edit.
Shared history contains applied/reverted content only and fails closed at the
documented row and byte budgets until cursor pagination lands.
The remaining plan completes the product domain and integrations, connects the
frontend, hardens the finished system, and only then creates the private
production environment; see
[the ordered implementation plan](docs/DESIGN.md#12-implementation-plan).

The backend has one runtime persistence implementation: DynamoDB. Development
authentication does not select volatile storage; until local DynamoDB support
is added, running the API requires an explicitly configured table and AWS SDK
configuration. Fast API tests use test-target-only fakes that cannot enter the
application binary.

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
