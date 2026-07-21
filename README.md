# Itinera

*Latin: the plural of **iter** — journeys, roads.*

Itinera is a collaborative trip-planning app for small groups of friends. Plans live
on a map, changes happen through polls, expenses are settled in a shared ledger, and
your AI assistant can join the planning through short-lived, scoped API tokens.

## Documents

- [Design document](docs/DESIGN.md) — architecture, data model, and product design.

## Tech at a glance

| Layer    | Choice                                                        |
|----------|---------------------------------------------------------------|
| Backend  | Rust (axum) on AWS Lambda                                     |
| Frontend | TypeScript + React (Vite), hosted on Cloudflare Pages         |
| Database | Postgres (Neon free tier) behind repository traits            |
| Maps     | Google Maps Platform (Essentials tier) behind provider traits |
| Auth     | Email one-time codes via Amazon SES, session cookies          |

**Design rule #1:** every external service sits behind an interface (Rust trait /
TypeScript interface) so providers can be swapped without touching callers.
