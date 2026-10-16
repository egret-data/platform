use egret_policy::infra_names::{
    is_egret_infrastructure_policy, ADMIN_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_NAME_PREFIX,
    EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
    SELF_IMPERSONATE_POLICY_NAME,
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
    assert!(is_egret_infrastructure_policy(
        "trino",
        "egret-admin - catalog, schema, table"
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        "egret-admin - catalog, schema, table, column"
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        "all - catalog, schema, table"
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        IMPERSONATE_POLICY_NAME
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        QUERY_TIMEOUT_POLICY_NAME
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        SELF_IMPERSONATE_POLICY_NAME
    ));
    assert!(is_egret_infrastructure_policy(
        "trino",
        EXECUTE_QUERY_POLICY_NAME
    ));

    // Negatives — an operator's own names, including ones that start with the
    // same words. The seeder's format is `"{prefix} - {chain}"`, so the
    // separator is load-bearing and a bare prefix match is too wide.
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "all-hands-reporting"
    ));
    assert!(!is_egret_infrastructure_policy("trino", "allow sales"));
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "egret-admin-notes"
    ));
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "egret-admins - catalog"
    ));
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "egret-query-timeouts"
    ));
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "egret-user-impersonate-selfish"
    ));
    assert!(!is_egret_infrastructure_policy(
        "trino",
        "egret-user-execute-query-2"
    ));
    assert!(!is_egret_infrastructure_policy("trino", ""));
    assert!(!is_egret_infrastructure_policy("trino", "perm-grant-abc"));
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
    assert_eq!(SELF_IMPERSONATE_POLICY_NAME, "egret-user-impersonate-self");
    assert_eq!(EXECUTE_QUERY_POLICY_NAME, "egret-user-execute-query");
}

/// SP4d-3 final review I-1 (USER RULING, narrowed): only the `all - ` shape is
/// Trino-only. The default set is built from the Trino def, so on any other
/// type an `all - …` name is an operator's; the per-admin set and the
/// delegator's two names are egret's on every type (StarRocks profiles get a
/// StarRocks-shaped `egret-admin - …` set, and unlocking it would let an
/// operator lock admins out).
#[test]
fn only_the_default_all_shape_depends_on_the_service_type() {
    for ty in ["starrocks", "tag", "trino_seed_custom", ""] {
        assert!(
            !is_egret_infrastructure_policy(ty, "all - catalog, schema, table"),
            "{ty:?}"
        );
        assert!(
            is_egret_infrastructure_policy(ty, "egret-admin - catalog, database, table"),
            "{ty:?}"
        );
        assert!(
            is_egret_infrastructure_policy(ty, IMPERSONATE_POLICY_NAME),
            "{ty:?}"
        );
        assert!(
            is_egret_infrastructure_policy(ty, QUERY_TIMEOUT_POLICY_NAME),
            "{ty:?}"
        );
        assert!(
            is_egret_infrastructure_policy(ty, SELF_IMPERSONATE_POLICY_NAME),
            "{ty:?}"
        );
        assert!(
            is_egret_infrastructure_policy(ty, EXECUTE_QUERY_POLICY_NAME),
            "{ty:?}"
        );
    }
    // The control: the same name on Trino IS infrastructure.
    assert!(is_egret_infrastructure_policy(
        "trino",
        "all - catalog, schema, table"
    ));
    // Case and near-misses of the type are not Trino.
    assert!(!is_egret_infrastructure_policy("Trino", "all - catalog"));
    assert!(!is_egret_infrastructure_policy("trino ", "all - catalog"));
}
