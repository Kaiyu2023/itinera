//! Domain core: Itinera's types, port traits, and the services built on them.
//!
//! **This crate takes no vendor dependency** — no HTTP client, no database
//! driver, no cloud SDK, ever. Anything external is expressed here as a *port*
//! (a trait) and implemented in `itinera-adapters`, so swapping a provider is
//! an adapter change and never a change to the domain. If you find yourself
//! reaching for a vendor crate in this `Cargo.toml`, the dependency belongs on
//! the far side of a port instead. See DESIGN.md §2.1.
