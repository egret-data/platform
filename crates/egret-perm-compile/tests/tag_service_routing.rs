//! SP4a Task 11, defect 2: a Trino tag policy must be STORED under the tag
//! service its own body names — and must still be REAPED from there when its
//! rule is deleted.
//!
//! Task 10 measured a compiled tag mask that returned `book`, `pen`, `lamp`,
//! `desk` — the real values — from a column the model said was masked
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, T3).
//! The compiled body was correct Ranger, verbatim. The ROW carried the trino
//! profile's `service_id` instead of the tag service's, so
//! `GET /service/plugins/policies/download/<profile>_tag` returned **0
//! policies** while the resource profile carried a `serviceType: tag` policy it
//! can never match against a `catalog/schema/table/column` request.
//!
//! **Every existing compile test passed over this, because every one of them
//! asserted the JSON body.** The body was already right. So the assertions here
//! are `policy.authz_policies.service_id` and what the download function
//! actually serves — a body-shaped test would pass against the bug being fixed.
//!
//! # Why this is its own test BINARY, and one test
//!
//! `egret-test-db` gives each test **process** a private database. A Trino
//! tag-selected rule is catalog-UNSCOPED by design (D5: the plugin resolves the
//! tag, so the rule applies wherever the tag turns up), which means every Trino
//! profile in the database compiles every tag rule in it. In a shared binary
//! that has two consequences a test cannot assert around:
//!
//! - another fixture's profile writes THIS rule's policy into ITS tag service,
//!   so "the policy is gone" is not answerable by a search on the name; and
//! - deleting a tag rule while a concurrent test's recompile is between its
//!   `list_rules` and its `perm.compiled` provenance insert violates
//!   `compiled_rule_id_fkey` — a race the production delete path closes by
//!   locking every affected profile first, and that a store-level delete in a
//!   test does not.
//!
//! A private database removes both at the root. The single test walks the whole
//! life of one rule (create → route → delete → reap) rather than splitting it
//! into two tests that would then race with each other inside this binary for
//! exactly the same reason.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test tag_service_routing

use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Selector};
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
use serde_json::Value;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// The id of the policy named `name` under EXACTLY `service_id`, or `None`.
///
/// `(service_id, name)` is the pair `policy.authz_policies`' own UNIQUE
/// constraint is on, and asking about a named service is the entire point: the
/// bug put the RIGHT body under the WRONG `service_id`, so a lookup by name
/// alone would have found it and reported success.
async fn policy_id_in(pool: &sqlx::PgPool, service_id: i64, name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2")
        .bind(service_id)
        .bind(name)
        .fetch_optional(pool)
        .await
        .expect("policy by (service_id, name)")
}

async fn policy_version(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("policy_version")
}

#[tokio::test]
async fn a_trino_tag_rule_lands_in_the_tag_service_and_is_reaped_from_it_on_delete() {
    let Some(db) = test_db().await else { return };
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());

    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_tagroute");
    let tag_service_name = format!("{profile_name}_tag");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create trino profile");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");

    let tag_key = unique("route-pii");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, 'Route PII')")
        .bind(&tag_key)
        .execute(pool)
        .await
        .expect("insert tag");

    let rule_id = perm
        .create_rule(&Rule {
            id: Uuid::nil(),
            name: unique("route-rule"),
            kind: RuleKind::Mask {
                mask_type: "MASK".to_string(),
                value_expr: None,
            },
            selector: Selector::Tag {
                key: tag_key.clone(),
            },
            exempt: PrincipalSet::default(),
        })
        .await
        .expect("create rule");

    let tag_version_before = policy_version(pool, tag_service.id).await;
    let profile_version_before = policy_version(pool, profile.id).await;

    recompile_service(&perm, &policy, profile.id)
        .await
        .expect("recompile");

    // ── 1. routing ───────────────────────────────────────────────────────────
    let policy_name = format!("perm-rule-{rule_id}");
    let row_id = policy_id_in(pool, tag_service.id, &policy_name)
        .await
        .unwrap_or_else(|| {
            panic!(
                "the tag policy must be STORED under the tag service ({}) its own body names — \
                 filed under the trino profile ({}) instead, it deploys, is listed, and \
                 enforces nothing (Task 10, case T3)",
                tag_service.id, profile.id
            )
        });
    assert_eq!(
        policy_id_in(pool, profile.id, &policy_name).await,
        None,
        "the resource profile must not carry a serviceType:tag policy it can never match"
    );

    // The body was ALWAYS right — asserted here so a later change cannot
    // "fix the routing" by rewriting the body instead.
    let body: Value =
        sqlx::query_scalar("SELECT policy_jsonb FROM policy.authz_policies WHERE id = $1")
            .bind(row_id)
            .fetch_one(pool)
            .await
            .expect("body");
    assert_eq!(body["serviceType"], "tag");
    assert_eq!(body["service"], tag_service_name);
    assert_eq!(body["resources"]["tag"]["values"][0], "Route PII");
    // ⚠ `policyPriority` MUST be 0 on every tag policy egret writes, and until
    // SP4d-2b Task 5 fix round 1 (MINOR 9) nothing asserted it.
    //
    // The differential gate measured that Ranger arbitrates the tag phase by
    // `policyPriority` through a per-access-type result map it does not model, so
    // `egret-ranger-eval` REFUSES a tag block whose deciding policies sit at more
    // than one priority
    // (`UndecidableCause::TagPriorityArbitrationNotModelled`). That refusal was
    // accepted on the strength of "it costs nothing, because egret never emits a
    // non-zero priority on a tag policy" — a cost claim resting on three
    // hard-coded `is_override: false`s (`clearance.rs`, and both arms of
    // `rules.rs::build_tag_body`) with no test between them and the claim.
    // `AccessPolicyInput.is_override` is a PUBLIC wire field (`isOverride`), so one
    // flag flip makes egret author a multi-priority tag block and turns that
    // refusal live on egret's own data.
    //
    // Asserted on the emitted `policyPriority` rather than on the flag: the
    // invariant is about what the plugin downloads, and it survives a refactor
    // that renames or reroutes `is_override`.
    assert_eq!(
        body["policyPriority"], 0,
        "a tag policy at a non-zero priority makes egret's own downloads reach \
         `TagPriorityArbitrationNotModelled`, and the zero-cost claim behind that \
         refusal is exactly this value"
    );

    // ── 1b. the tag service is CLAIMED, like the profile ─────────────────────
    // Whole-branch review, Important 5. Task 8 stamped `origin='generated'` on
    // the profile alone; Task 11 then moved compiled tag policies into the tag
    // service, which stayed `manual` and therefore still accepted a
    // hand-written policy through `create_policy`. Ranger evaluates tag
    // policies FIRST and short-circuits, so a hand-written tag ACCESS allow in
    // this very service ends evaluation before any compiled deny grant is
    // consulted — D2's "egret never emits a tag access policy" held for the
    // compiler and not for egret.
    let origin_of = |service_id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT origin FROM policy.authz_services WHERE id = $1",
            )
            .bind(service_id)
            .fetch_one(&pool)
            .await
            .expect("origin")
        }
    };
    assert_eq!(
        origin_of(tag_service.id).await,
        "generated",
        "a tag service the compiler writes into must carry the same origin treatment as the \
         profile it serves"
    );
    assert_eq!(origin_of(profile.id).await, "generated");
    let refused = policy
        .create_policy(
            tag_service.id,
            "hand-written-tag-allow",
            &serde_json::json!({
                "name": "hand-written-tag-allow",
                "serviceType": "tag",
                "policyType": 0,
                "resources": { "tag": { "values": ["Route PII"] } },
            }),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(egret_policy::PolicyStoreError::CompilerOwned(_))
        ),
        "a hand-written tag policy in the compiler's own tag service must be refused, not \
         evaluated ahead of every compiled deny; got {refused:?}"
    );

    // ── 2. both versions bump ────────────────────────────────────────────────
    // The tag plugin polls the TAG service with `lastKnownVersion`; without its
    // bump it is answered 304 and never sees the mask. The resource plugin polls
    // the PROFILE, whose download embeds the tag service's policies as
    // `tagPolicies` and is gated by the profile's own version
    // (`egret-policy/src/ranger/api/download.rs`) — so a tag-only change that
    // left the profile's version alone would 304 that plugin too.
    assert_eq!(
        policy_version(pool, tag_service.id).await,
        tag_version_before + 1,
        "writing under the tag service must bump THAT service's policy_version"
    );
    assert_eq!(
        policy_version(pool, profile.id).await,
        profile_version_before + 1,
        "the profile's version must bump too, or its plugin is answered 304 and never \
         re-downloads the tagPolicies block"
    );

    // ── 3. the download actually serves it ───────────────────────────────────
    // `enabled_policies_for_service` is the function the Ranger download
    // endpoint itself calls for both `/policies/download/{service}` and the
    // embedded `tagPolicies` block.
    let served = policy
        .enabled_policies_for_service(tag_service.id)
        .await
        .expect("tag service download");
    assert!(
        served.iter().any(|p| p["name"] == policy_name.as_str()),
        "the tag service's own download must carry the mask"
    );

    // ── 4. the sweep reaps it across services ────────────────────────────────
    // `apply_diff_in_tx` enumerates existing state by service id + this
    // compiler's name prefixes. Now that the policy lives in a DIFFERENT service
    // than the profile being recompiled, a sweep scoped to the profile alone
    // would never see it again — Task 7's own fix-round-1 Critical (a deleted
    // rule's policy enforced forever) re-created one service over. The sweep set
    // therefore comes from the profile's CONFIGURATION, not from what compiled,
    // so the tag service is enumerated even on the recompile where nothing is
    // desired for it at all.
    perm.delete_rule(rule_id).await.expect("delete rule");

    recompile_service(&perm, &policy, profile.id)
        .await
        .expect("recompile after delete");

    assert_eq!(
        policy_id_in(pool, tag_service.id, &policy_name).await,
        None,
        "the deleted rule's policy must be reaped from the TAG service — a sweep scoped to \
         the profile alone would leave it enforced there forever"
    );
    let served_after = policy
        .enabled_policies_for_service(tag_service.id)
        .await
        .expect("tag service download");
    assert!(
        served_after
            .iter()
            .all(|p| p["name"] != policy_name.as_str()),
        "and the tag service's own download must no longer serve it"
    );
}
