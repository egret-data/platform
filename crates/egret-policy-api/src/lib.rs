//! Policy/Ranger seam contract (E2 contract-crate seed): the engine-facing wire
//! DTOs + the `PolicyPort` trait that cross-seam callers depend on instead of the
//! concrete `PolicyStore`. Leaf crate — serde-only, no sqlx/axum/egret-* deps.
pub mod dto;
pub mod infra_names;
pub mod port;

pub use dto::{PolicyService, ServicePolicies, ServiceTags, TagAssignment};
pub use port::{PolicyError, PolicyPort};

/// The `policy.authz_services.config_jsonb` key linking a resource profile to the
/// TAG service its tag-based masks / row filters / clearances live in.
///
/// It lives in this leaf crate rather than in `egret-policy` because it is read on
/// BOTH sides of the policy seam and written from a third crate: `egret-policy`
/// (`download_policies`' `tagPolicies` block, `create_profile`, the tag-editor
/// resolver, `refuse_tag_service_on_non_trino_profile`), `egret-perm-compile`
/// (`trigger`'s compile target, `explain`'s download view) and
/// `egret-engine-mgmt` (`bind_trino_profile`, the only producer of a value). Those
/// three already depend on this crate; a const in `egret-policy` could not reach
/// `egret-engine-mgmt` without inverting the `PolicyPort` seam.
///
/// # ⚠ What this const does NOT cover, and why
///
/// Four SQL statements embed the key as a literal inside a query string and
/// deliberately keep doing so — interpolating a Rust const into SQL means building
/// the statement with `format!`, which is the dynamic-SQL shape this repository
/// avoids, and the gain would be nil because the const cannot change without those
/// queries being rewritten anyway:
///
/// | statement | file |
/// |---|---|
/// | `usable_tag_service` | `egret-policy/src/store.rs` |
/// | `tag_assignments_for_service` (`tp.type = 'trino' AND tp.config_jsonb->>…`) | `egret-policy/src/store.rs` |
/// | `services_and_tag_services_via` | `egret-policy/src/store.rs` |
/// | `bump_bound_tag_versions`' id resolution (two clauses, one query) | `egret-catalog-registry/src/tag_version.rs` |
///
/// Plus migration `0176_tag_service_requires_schema_level.sql`, whose `CHECK` and
/// clearing `UPDATE` both name it. So: **every Rust occurrence of the key in `src/`
/// goes through this const; the SQL occurrences are the four above plus the
/// migration, and they are enumerated here rather than left to be discovered.**
///
/// TEST FIXTURES ARE THE OTHER DELIBERATE EXCEPTION. The unit tests in
/// `egret-policy/src/store.rs` spell `"tagService"` out in their `json!` inputs
/// instead of using this const, on purpose: a test that builds its input from the
/// same const the code under test reads agrees with that const by construction
/// and so cannot catch it being wrong. The literal is the independent half of the
/// oracle. `the_const_is_the_key_the_sql_and_the_migration_name` is what pins the
/// const to that literal, and therefore to the five statements above that cannot
/// reference it.
pub const TAG_SERVICE_CONFIG_KEY: &str = "tagService";
