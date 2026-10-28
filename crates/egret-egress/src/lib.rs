//! Requests to URLs a user chose.
//!
//! Two parts, used together:
//!
//! - [`guard`]: is this URL safe to send to? Parses the URL, resolves the host,
//!   and requires every resolved address to be public (or the host to be on an
//!   operator allowlist). Returns the vetted addresses so the caller can pin
//!   them.
//! - [`client`]: an HTTP executor built on the guard. It pins the vetted
//!   addresses, never uses a proxy, and follows redirects by hand so that every
//!   hop goes through the guard again.
//!
//! The crate depends on no other egret crate, so both `egret-automation` and
//! `egret-workflow` can use it without one depending on the other.

pub mod client;
pub mod guard;

pub use client::{EgressClient, EgressError, EgressOptions, RequestSpec};
pub use guard::{
    blocked_reason, vet, vet_url, DestinationError, Resolver, SsrfPolicy, SystemResolver,
};
