//! Task 3: `attribution::attribute_policy_name` maps a compiled Ranger policy
//! NAME back to the `perm` model object that minted it. Pure function, no DB.
//!
//! One test per shape the compiler actually mints (read at the minting
//! sites, not assumed): `grants.rs:451`, `rules.rs:339`/`:445` (bare rule),
//! `rules.rs:317-326` (StarRocks per-column tag expansion, row-filter and
//! mask arities), `clearance.rs:307` (bare clearance),
//! `clearance.rs:354` (StarRocks per-column clearance expansion) — plus every
//! no-model-object shape that is LIVE in the cluster today and must come
//! back `None` cleanly rather than erroring or matching wrong.

use egret_perm_compile::attribution::{attribute_policy_name, PolicyAttribution};
use egret_policy::{COMPILER_CLEARANCE_PREFIX, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Positive: every shape the compiler mints.
// ---------------------------------------------------------------------------

#[test]
fn grant_bare_uuid() {
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_GRANT_PREFIX}{id}");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Grant(id))
    );
}

#[test]
fn rule_bare_uuid_path_addressed() {
    // rules.rs:339 — a non-tag (path-addressed) mask/row-filter rule.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Rule { id, locator: None })
    );
}

#[test]
fn rule_bare_uuid_tag_addressed() {
    // rules.rs:445 — a tag-addressed (Trino) mask/row-filter rule. Same bare
    // shape as the path-addressed one; the compiler mints no locator here.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Rule { id, locator: None })
    );
}

#[test]
fn rule_per_column_row_filter_locator_three_part() {
    // rules.rs:317-326, RowFilter arm: suffix is `catalog.schema.table` (no
    // column — `rowFilterDef.resources` stops at table).
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}-analytics.public.orders");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Rule {
            id,
            locator: Some("analytics.public.orders".to_string()),
        })
    );
}

#[test]
fn rule_per_column_mask_locator_four_part() {
    // rules.rs:317-326, Mask arm: suffix is `catalog.schema.table.column`.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}-analytics.public.orders.ssn");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Rule {
            id,
            locator: Some("analytics.public.orders.ssn".to_string()),
        })
    );
}

#[test]
fn rule_locator_survives_hyphenated_catalog_name() {
    // A catalog name can contain hyphens (the brief's stated trap): the
    // boundary between the fixed-width uuid and the locator must not be
    // found by splitting on `-`.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}-sp3c2-trino-live.public.orders.ssn");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Rule {
            id,
            locator: Some("sp3c2-trino-live.public.orders.ssn".to_string()),
        })
    );
}

#[test]
fn clearance_bare_tag_id() {
    // clearance.rs:307 — Trino: one tag-service DENY per gated tag.
    let tag_id = Uuid::new_v4();
    let name = format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Clearance {
            tag_id,
            locator: None,
        })
    );
}

#[test]
fn clearance_per_column_locator() {
    // clearance.rs:354-356 — StarRocks per-column expansion.
    let tag_id = Uuid::new_v4();
    let name = format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}-analytics.public.customers.email");
    assert_eq!(
        attribute_policy_name(&name),
        Some(PolicyAttribution::Clearance {
            tag_id,
            locator: Some("analytics.public.customers.email".to_string()),
        })
    );
}

#[test]
fn the_engine_wide_materialized_view_deny_is_attributed() {
    // clearance.rs `mv_deny_inputs` (view governance A3, Task 3 review M-4):
    // one `perm-clearance-mv` per StarRocks profile, which carries no tag id.
    assert_eq!(
        attribute_policy_name(&format!("{COMPILER_CLEARANCE_PREFIX}mv")),
        Some(PolicyAttribution::MaterializedViewDeny)
    );
    // Only that exact name: a first-draft `…mv-{catalog}.{database}` is not it.
    assert_eq!(
        attribute_policy_name(&format!("{COMPILER_CLEARANCE_PREFIX}mv-ice.db1")),
        None
    );
}

// ---------------------------------------------------------------------------
// Negative: live, no-model-object shapes that must return `None` cleanly.
// ---------------------------------------------------------------------------

#[test]
fn seeded_egret_admin_policy_is_not_attributed() {
    // default_policies.rs: thirteen hierarchies per generated Trino profile,
    // granting `*` on everything, with no `perm.grants` row behind them.
    assert_eq!(
        attribute_policy_name("egret-admin - catalog, schema, table, column"),
        None
    );
}

#[test]
fn seeded_all_policy_is_not_attributed() {
    assert_eq!(
        attribute_policy_name("all - catalog, schema, table, column"),
        None
    );
}

#[test]
fn delegator_impersonate_policy_is_not_attributed() {
    // delegator_policy.rs — plumbing, never a data grant, but must not choke.
    assert_eq!(attribute_policy_name("egret-delegator-impersonate"), None);
}

#[test]
fn delegator_query_timeout_policy_is_not_attributed() {
    assert_eq!(attribute_policy_name("egret-query-timeout"), None);
}

#[test]
fn per_user_baseline_policies_are_not_attributed() {
    assert_eq!(attribute_policy_name("egret-user-impersonate-self"), None);
    assert_eq!(attribute_policy_name("egret-user-execute-query"), None);
}

#[test]
fn operator_written_raw_policy_is_not_attributed() {
    // Arbitrary name an operator typed directly into a `manual` profile.
    assert_eq!(attribute_policy_name("nightly-audit-row-filter"), None);
}

#[test]
fn superuser_short_circuit_is_not_a_policy() {
    // Ranger's super-user short-circuit reports `policy_id: Some(-1),
    // policy_name: "superuser"` — not a policy at all.
    assert_eq!(attribute_policy_name("superuser"), None);
}

// ---------------------------------------------------------------------------
// Mutation-guard: a wrong prefix or a truncated/garbled uuid must not
// silently produce a plausible attribution.
// ---------------------------------------------------------------------------

#[test]
fn truncated_uuid_after_grant_prefix_is_not_attributed() {
    let id = Uuid::new_v4();
    let truncated = &id.to_string()[..30];
    let name = format!("{COMPILER_GRANT_PREFIX}{truncated}");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn truncated_uuid_after_rule_prefix_is_not_attributed() {
    let id = Uuid::new_v4();
    let truncated = &id.to_string()[..30];
    let name = format!("{COMPILER_RULE_PREFIX}{truncated}");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn garbled_uuid_same_length_is_not_attributed() {
    // 36 characters, right shape, not a valid uuid.
    let name = format!("{COMPILER_GRANT_PREFIX}zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn grant_name_with_trailing_locator_is_not_attributed() {
    // A grant never mints a locator; anything after the bare uuid makes this
    // not a `perm-grant-` name at all, however plausible it looks.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_GRANT_PREFIX}{id}-extra");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn rule_name_missing_locator_separator_is_not_attributed() {
    // 36-char uuid followed by something that is not a `-` separator: must
    // not be swallowed into an ad-hoc locator.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}extra");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn rule_name_with_empty_locator_is_not_attributed() {
    // A trailing bare `-` with nothing after it is not a valid locator.
    let id = Uuid::new_v4();
    let name = format!("{COMPILER_RULE_PREFIX}{id}-");
    assert_eq!(attribute_policy_name(&name), None);
}

#[test]
fn wrong_prefix_case_is_not_attributed() {
    let id = Uuid::new_v4();
    let name = format!("Perm-Grant-{id}");
    assert_eq!(attribute_policy_name(&name), None);
}
