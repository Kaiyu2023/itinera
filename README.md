# Itinera

*Latin: the plural of **iter** — journeys, roads.*

Itinera is a collaborative trip-planning app I wrote for planning trips with my
friends: one shared itinerary on a map, candidate places we pitch and vote on,
structural plan changes that go through a poll or a leader's approval instead of
silently rewriting the plan, a shared expense ledger, and a "before you go" board
for the boring-but-important prep. It's built for our trips first — but if you
find it interesting or useful, you're free to use it, fork it, or take pieces of
it (MIT licensed).

## Documents

- [Design document](docs/DESIGN.md) — architecture, data model, and product design.
- [API contract](docs/openapi.yaml) — the single source of truth for the backend API.

## Tech at a glance

| Layer    | Choice                                                        |
|----------|---------------------------------------------------------------|
| Backend  | Rust (axum) on AWS Lambda                                     |
| Frontend | TypeScript + React (Vite), hosted on Cloudflare Pages         |
| Database | Postgres (Neon free tier) behind repository traits            |
| Maps     | Google Maps Platform (Essentials tier) behind provider traits |
| Auth     | Cloudflare Access one-time PIN login (free ≤ 50 users)          |

**Design rule #1:** every external service sits behind an interface (Rust trait /
TypeScript interface) so providers can be swapped without touching callers.

**Workflow:** frontend first (Claude), built against a `MockApiClient` and fixture
data; once the frontend is frozen, its API contract (`docs/openapi.yaml`) becomes
the spec for the Rust backend (Kaiyu), and `HttpApiClient` swaps in.

## Status

Phase A (working): the full frontend against the in-memory mock, with realistic
fixture data and a Playwright suite covering both desktop and mobile viewports.
Phase B (in progress): the Rust backend implementing `docs/openapi.yaml`.

## Development

```sh
cd frontend
npm install        # also wires up the pre-commit hook
npm run dev        # dev server
npm run test:e2e   # Playwright suite (desktop + mobile projects)
npm run typecheck  # tsc
npm run format     # prettier
```

The pre-commit hook runs the typecheck and a prettier check; CI enforces both,
plus the full e2e suite, on every PR.

## Contributing

This is a personal project shared with friends; collaborators are added by
invitation. Issues and forks are welcome, but expect PRs from outside the group
to move slowly — it's a hobby, not a product.

## Secrets

No credentials of any kind live in this repository — not in code, config,
fixtures, or CI files. Deployment credentials are provided exclusively through
GitHub Actions environment secrets (and OIDC where possible). If you fork this
to deploy your own, bring your own secrets the same way.

## License & assets

Code is [MIT](LICENSE). The place photos are from Wikimedia Commons under their
individual licenses — see
[`frontend/public/ATTRIBUTIONS.md`](frontend/public/ATTRIBUTIONS.md).
