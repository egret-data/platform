//! Is this webhook URL safe to send to?
//!
//! The guard now lives in the shared `egret-egress` crate (workflow package
//! #4, C1), so alert webhooks and the workflow `http.*` tasks are judged by
//! one range table. This module re-exports it so existing paths
//! (`deliver::destination::{vet, SsrfPolicy, Resolver, SystemResolver}`)
//! keep working. The guard's tests moved with it.
//!
//! Its error texts no longer carry the word "webhook";
//! [`super::webhook::WebhookError`] prefixes it, so a delivery failure reads
//! exactly as before.

pub use egret_egress::guard::{
    blocked_reason, vet, DestinationError, Resolver, SsrfPolicy, SystemResolver,
};
