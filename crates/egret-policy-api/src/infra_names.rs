//! The ONLY place egret's own infrastructure policy names are spelled.
//!
//! Four shapes exist, from two producers:
//!
//! 1. `egret_policy::default_policies::build_default_policies` mints
//!    `"{prefix} - {resource, chain, ...}"`, once per resource hierarchy, with
//!    the prefix [`ADMIN_ALL_ACCESS_NAME_PREFIX`] (the per-admin seeding) or
//!    [`DEFAULT_ALL_ACCESS_NAME_PREFIX`] (the group-granted default set).
//! 2. `egret_engine_mgmt::delegator_policy` mints exactly two fixed names,
//!    [`IMPERSONATE_POLICY_NAME`] and [`QUERY_TIMEOUT_POLICY_NAME`].
//!
//! **Why this list lives here and not beside either producer.** The predicate
//! has to cover both, and this leaf contract crate is the only one all three
//! consumers can import in production: `egret-engine-mgmt` reaches the policy
//! seam only through `egret-policy-api` (`egret-policy` is a dev-dependency
//! there by design), and `egret-policy` and `egret-perm-compile` depend on it
//! too. It is re-exported as `egret_policy::infra_names`. A copy of this list
//! anywhere else would be a second spelling of one fact, which is the failure
//! this programme reproduced five times on the branch merged as #656 — two
//! guards that must agree, written to different shapes.
//!
//! **This predicate is a BACKFILL and a CROSS-CHECK, not the enforcement.**
//! Enforcement is `policy.authz_policies.managed_by`, a column, because a
//! predicate over a name is defeated by a rename and a column is not. This
//! function exists to compute the column's initial value and to assert, in a
//! test, that the column and the names still agree.

/// Name prefix of the per-admin all-access policies
/// (`seed_admin_all_access` → `build_default_policies(..., "egret-admin", ...)`).
pub const ADMIN_ALL_ACCESS_NAME_PREFIX: &str = "egret-admin";

/// Name prefix of the group-granted default policy set
/// (`seed_default_trino_policies` → `build_default_policies(..., "all", ...)`).
///
/// ⚠ Do not confuse this with the access-type alias **also** spelled `all`,
/// which `default_policies::all_access_types` filters out of a grant set. That
/// one is an entry in the service-def's `accessTypes`; this one is a policy
/// name prefix. They are unrelated and a sweep that treats them as one string
/// breaks the access-type filter without failing a test.
pub const DEFAULT_ALL_ACCESS_NAME_PREFIX: &str = "all";

/// The delegator's impersonation policy. Moved here from
/// `egret_engine_mgmt::delegator_policy` so this list has one home; that module
/// re-exports it, so its own callers are unchanged.
pub const IMPERSONATE_POLICY_NAME: &str = "egret-delegator-impersonate";

/// The delegator's query-timeout policy. Same move, same reason.
pub const QUERY_TIMEOUT_POLICY_NAME: &str = "egret-query-timeout";

/// The separator `build_default_policies` puts between the prefix and the
/// resource chain: `format!("{} - {}", name_prefix, names.join(", "))`. Matching
/// on the prefix alone would claim an operator's `"all-hands-reporting"` as
/// infrastructure and lock them out of it.
const SEEDED_NAME_SEPARATOR: &str = " - ";

/// Whether `name` is a policy egret's own plumbing minted.
pub fn is_egret_infrastructure_policy_name(name: &str) -> bool {
    name == IMPERSONATE_POLICY_NAME
        || name == QUERY_TIMEOUT_POLICY_NAME
        || is_seeded_hierarchy_name(name, ADMIN_ALL_ACCESS_NAME_PREFIX)
        || is_seeded_hierarchy_name(name, DEFAULT_ALL_ACCESS_NAME_PREFIX)
}

fn is_seeded_hierarchy_name(name: &str, prefix: &str) -> bool {
    match name.strip_prefix(prefix) {
        Some(rest) => rest.starts_with(SEEDED_NAME_SEPARATOR),
        None => false,
    }
}
