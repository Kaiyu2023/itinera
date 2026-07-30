//! Adapters: concrete implementations of the ports declared in `itinera-core`.
//!
//! One module per provider — Google Maps (`PlaceCatalog`, `RoutingEngine`),
//! Postgres (the repositories), Cloudflare Access (`IdentityProvider`), R2
//! (`BlobStore`). Vendor SDKs and their types stay behind this boundary: a
//! port's signature must never expose them, or the swap it exists to enable
//! stops being possible. See DESIGN.md §2.1.

pub mod insecure;
pub mod memory;
pub mod uuid_ids;
