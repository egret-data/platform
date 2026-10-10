use egret_policy::infra_names::{
    is_egret_infrastructure_policy_name, ADMIN_ALL_ACCESS_NAME_PREFIX,
    DEFAULT_ALL_ACCESS_NAME_PREFIX, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
};

/// The four shapes the seeder and the delegator actually mint, and the
/// near-misses that must NOT be claimed as infrastructure. The negatives are
/// the half that matters: a predicate that answers `true` for everything would
/// pass the positives alone and would then lock an operator out of their own
/// policies.
#[test]
fn the_four_infrastructure_shapes_are_recognised_and_nothing_else_is() {
    // Positives — exactly what `default_policies::build_default_policies` and
    // `delegator_policy` produce.
    assert!(is_egret_infrastructure_policy_name(
        "egret-admin - catalog, schema, table"
    ));
    assert!(is_egret_infrastructure_policy_name(
        "egret-admin - catalog, schema, table, column"
    ));
    assert!(is_egret_infrastructure_policy_name(
        "all - catalog, schema, table"
    ));
    assert!(is_egret_infrastructure_policy_name(IMPERSONATE_POLICY_NAME));
    assert!(is_egret_infrastructure_policy_name(
        QUERY_TIMEOUT_POLICY_NAME
    ));

    // Negatives — an operator's own names, including ones that start with the
    // same words. The seeder's format is `"{prefix} - {chain}"`, so the
    // separator is load-bearing and a bare prefix match is too wide.
    assert!(!is_egret_infrastructure_policy_name("all-hands-reporting"));
    assert!(!is_egret_infrastructure_policy_name("allow sales"));
    assert!(!is_egret_infrastructure_policy_name("egret-admin-notes"));
    assert!(!is_egret_infrastructure_policy_name(
        "egret-admins - catalog"
    ));
    assert!(!is_egret_infrastructure_policy_name("egret-query-timeouts"));
    assert!(!is_egret_infrastructure_policy_name(""));
    assert!(!is_egret_infrastructure_policy_name("perm-grant-abc"));
}

/// The constants are the values the seeder passes today. If someone changes a
/// literal in `default_policies.rs` without changing the constant, this fails —
/// which is the only thing stopping the predicate and the producer from
/// drifting apart.
#[test]
fn the_constants_are_the_values_the_producers_use() {
    assert_eq!(ADMIN_ALL_ACCESS_NAME_PREFIX, "egret-admin");
    assert_eq!(DEFAULT_ALL_ACCESS_NAME_PREFIX, "all");
    assert_eq!(IMPERSONATE_POLICY_NAME, "egret-delegator-impersonate");
    assert_eq!(QUERY_TIMEOUT_POLICY_NAME, "egret-query-timeout");
}
