//! `egret-dataapi-api` — the Data API seam's contract crate (P2).
//!
//! Mirrors `egret-query-api` / `egret-catalog-api` / `egret-policy-api`: a pure
//! leaf (no sqlx, axum, or heavy egret-* deps) so any cross-seam consumer can
//! depend on the Data API's wire vocabulary cheaply.
//!
//! Empty for now: Task 4 (this crate's introduction) only needs `egret-dataapi`
//! to exist and register a `ClientVerifier` with the machine-IdP. The
//! management-route DTOs (service-account / client request+response shapes)
//! land in Task 5 alongside `routes.rs`.
