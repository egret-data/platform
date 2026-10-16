//! Maps a compiled Ranger policy NAME back to the `perm` model object that
//! minted it (SP4d-2a Task 3) — a pure function over a string, and nothing
//! else. `explain`'s evaluator (`egret-ranger-eval`) reports a decision
//! attributed to a Ranger policy name and id; for the answer to stay
//! readable to a steward, a decision attributed to `perm-grant-{uuid}` should
//! be reported as that Grant.
//!
//! **The mapping is partial by design, and getting that right matters more
//! than the happy path.** Four categories of policy have NO model object and
//! are all live in the cluster today, so [`attribute_policy_name`] must
//! return `None` for every one of them, cleanly, rather than erroring or
//! guessing:
//!
//! 1. **Seeded infrastructure** — `egret_policy::default_policies` mints
//!    names like `"egret-admin - catalog, schema, table, column"` and
//!    `"all - catalog, schema, table, column"`, thirteen hierarchies per
//!    generated Trino profile, granting `*` on everything.
//! 2. **Delegator plumbing** — `egret_engine_mgmt::delegator_policy` has four
//!    stable names, `egret-delegator-impersonate` and the per-user baseline
//!    `egret-user-impersonate-self`, `egret-user-execute-query` and
//!    `egret-query-timeout`. None is a data grant, so none will ever decide a
//!    `select`, but this function must not choke on them.
//! 3. **Operator-written raw JSON** — arbitrary names, constrained to
//!    `manual` profiles but reachable by `explain`.
//! 4. **Ranger's super-user short-circuit** — the evaluator can report
//!    `policy_id: Some(-1), policy_name: "superuser"`, which is not a policy
//!    at all.
//!
//! The consumer names these by their Ranger name and id instead, and must
//! never drop them: dropping a policy egret's compiler does not own would
//! recreate, inside `explain`, the exact blindness this package exists to
//! remove.
//!
//! **Prefixes are the compiler's own** (`egret_policy::{COMPILER_GRANT_PREFIX,
//! COMPILER_RULE_PREFIX, COMPILER_CLEARANCE_PREFIX}`) — this module does not
//! keep a second copy of those strings, for the same reason `grants.rs`,
//! `rules.rs`, `clearance.rs`, and `trigger.rs` all import them rather than
//! re-typing them: two independent copies would drift.

use egret_policy::{COMPILER_CLEARANCE_PREFIX, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX};
use uuid::Uuid;

/// The compiler-owned model object a Ranger policy NAME attributes to.
///
/// - [`PolicyAttribution::Grant`] — `perm-grant-{uuid}` (`grants::compile_grant`,
///   `grants.rs:451`). Always a bare id; a grant never mints a locator suffix.
/// - [`PolicyAttribution::Rule`] — `perm-rule-{uuid}` (`rules::compile_rule`,
///   `rules.rs:339` for a path-addressed rule, `:445` for a Trino
///   tag-addressed one), or, for StarRocks' per-column tag expansion,
///   `perm-rule-{uuid}-{locator}` (`rules.rs:317-326`). **A per-column rule
///   expands ONE `perm.rules` row into N policies**, so this mapping is
///   many-to-one and `locator` is the only thing that says which of the N
///   expansions decided a given resource — kept verbatim (not re-split into
///   catalog/schema/table/column) because the row-filter and mask arities
///   already disagree (`catalog.schema.table` vs
///   `catalog.schema.table.column`) and further parsing is not this
///   function's job.
/// - [`PolicyAttribution::Clearance`] — `perm-clearance-{tag_id}`
///   (`clearance::compile_clearance`, `clearance.rs:307`), or, for
///   StarRocks' per-column expansion, `perm-clearance-{tag_id}-{locator}`
///   (`clearance.rs:354`). `tag_id` names a `tags.id` row — a third object
///   class, neither a grant nor a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAttribution {
    /// `perm-grant-{uuid}`.
    Grant(Uuid),
    /// `perm-rule-{uuid}` or `perm-rule-{uuid}-{locator}`.
    Rule { id: Uuid, locator: Option<String> },
    /// `perm-clearance-{tag_id}` or `perm-clearance-{tag_id}-{locator}`.
    /// `tag_id` is `tags.id`, resolved via `tags.key`/`tags.name` by the
    /// caller if a display name is wanted — this module never touches the
    /// tags table.
    Clearance {
        tag_id: Uuid,
        locator: Option<String>,
    },
    /// `perm-clearance-mv` (`clearance::mv_deny_inputs`, view governance A3):
    /// the engine-wide `create materialized view` deny of a StarRocks profile.
    /// It belongs to no single tag: its exceptions are the users cleared for
    /// every classification on the profile.
    MaterializedViewDeny,
}

/// A `Uuid`'s hyphenated `Display` (8-4-4-4-12 hex) is always exactly this
/// many bytes — verified, not assumed, by `uuid_display_is_36_bytes` below.
const UUID_DISPLAY_LEN: usize = 36;

/// Attribute a Ranger policy NAME to the compiled model object that produced
/// it, or `None` when the name carries no model attribution at all — see the
/// module doc for the four live categories that legitimately return `None`.
pub fn attribute_policy_name(name: &str) -> Option<PolicyAttribution> {
    if let Some(rest) = name.strip_prefix(COMPILER_GRANT_PREFIX) {
        return parse_bare_uuid(rest).map(PolicyAttribution::Grant);
    }
    if let Some(rest) = name.strip_prefix(COMPILER_RULE_PREFIX) {
        let (id, locator) = parse_uuid_and_locator(rest)?;
        return Some(PolicyAttribution::Rule { id, locator });
    }
    if let Some(rest) = name.strip_prefix(COMPILER_CLEARANCE_PREFIX) {
        if rest == crate::clearance::MV_DENY_SUFFIX {
            return Some(PolicyAttribution::MaterializedViewDeny);
        }
        let (tag_id, locator) = parse_uuid_and_locator(rest)?;
        return Some(PolicyAttribution::Clearance { tag_id, locator });
    }
    None
}

/// Split `rest` into its leading fixed-width `Uuid` and whatever bytes
/// follow, or `None` when the first [`UUID_DISPLAY_LEN`] bytes are absent
/// (too short — including a truncated uuid) or are not a valid hyphenated
/// uuid. This is the one place that boundary is drawn: never by searching for
/// a `-`, since a catalog name embedded in the locator can contain any
/// number of hyphens of its own.
fn split_uuid_prefix(rest: &str) -> Option<(Uuid, &str)> {
    let uuid_str = rest.get(..UUID_DISPLAY_LEN)?;
    let id = Uuid::parse_str(uuid_str).ok()?;
    Some((id, &rest[UUID_DISPLAY_LEN..]))
}

/// A grant name never carries anything past its uuid — a bare id or nothing.
fn parse_bare_uuid(rest: &str) -> Option<Uuid> {
    let (id, remainder) = split_uuid_prefix(rest)?;
    remainder.is_empty().then_some(id)
}

/// A rule/clearance name is either bare, or the uuid followed by exactly one
/// `-` and a non-empty locator. Anything else after the uuid (no separator,
/// or a separator with nothing after it) is refused rather than guessed at.
fn parse_uuid_and_locator(rest: &str) -> Option<(Uuid, Option<String>)> {
    let (id, remainder) = split_uuid_prefix(rest)?;
    if remainder.is_empty() {
        return Some((id, None));
    }
    let locator = remainder.strip_prefix('-')?;
    if locator.is_empty() {
        return None;
    }
    Some((id, Some(locator.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The brief's own instruction: verify the 36-byte assumption rather
    /// than assume it. `Uuid::Display`'s hyphenated form is
    /// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` — 32 hex digits + 4 hyphens.
    #[test]
    fn uuid_display_is_36_bytes() {
        assert_eq!(Uuid::nil().to_string().len(), UUID_DISPLAY_LEN);
        assert_eq!(Uuid::max().to_string().len(), UUID_DISPLAY_LEN);
        for _ in 0..16 {
            assert_eq!(Uuid::new_v4().to_string().len(), UUID_DISPLAY_LEN);
        }
    }
}
