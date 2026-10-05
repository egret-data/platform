//! SP4c **Task 3**: a clearance has THREE inputs, and each one's write must
//! produce the policy change immediately — in the same transaction, with no
//! second write and no manual compile.
//!
//! **Every assertion here reads `policy.authz_policies`, never the input row.**
//! That is the whole point. SP4a shipped "a write without its recompile" on four
//! separate paths, and each time the input row was written perfectly: the gate
//! was in `classification_policies`, the assignment in
//! `catalog_tag_assignments`, the membership in `access_grants`, and egret's UI
//! read all three and showed the control as in force while the engine enforced
//! the previous policy set. A test that asserts the input row was written passes
//! against every one of those defects.
//!
//! The three inputs, and which of these tests covers each:
//!
//! 1. **The tag's GATE** (`classification_policies`) —
//!    `setting_a_clearance_compiles_the_deny_with_no_second_write` and its
//!    lifting half. Before SP4c Task 3 this input reached the data plane through
//!    nothing at all.
//! 2. **Which columns carry the tag** (`catalog_tag_assignments`) —
//!    `tagging_a_column_extends_the_clearance_on_starrocks`. SP4a Task 7 already
//!    recompiles on this write; this VERIFIES it now covers clearance rather than
//!    assuming it, which on StarRocks it must, because there the clearance is a
//!    per-column expansion.
//! 3. Who holds the clearing role — covered in
//!    `egret-perm-compile/tests/clearance_triggers.rs`, which is where that
//!    trigger lives.
//!
//! Plus two writes that are ALSO writes to those inputs, both found by
//! re-deriving the list for this task instead of inheriting it:
//! `renaming_a_gated_tag_recompiles_its_clearance` (fail-OPEN: the affected set
//! for a rename was gated on `perm.rules` alone, and a gated tag needs no rule)
//! and `deleting_a_gated_tag_reaps_its_clearance` (a deny nobody could lift).
//!
//! # Why its own test BINARY, and why the tests inside it serialize
//!
//! `egret-test-db` gives each test PROCESS a private database, and a clearance is
//! installation-wide: each write here recompiles EVERY compiler-targeted profile
//! in the database. Sharing a binary with another fixture would mean recompiling
//! that fixture's profile from underneath it — the reason
//! `tag_rename_recompiles.rs` stands alone. Within this binary the tests hold a
//! one-permit semaphore, because a tag DELETE's installation-wide recompile
//! running concurrently with another test's assertion is a race in the harness,
//! not behaviour.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_bi::{ClassificationPolicyRepo, TagRepo};
use egret_events::EventBus;
use egret_metastore::Db;
use egret_policy::{PolicyStore, COMPILER_CLEARANCE_PREFIX};
use serde_json::Value;
use uuid::Uuid;

/// One permit: see the module doc comment.
static SERIAL: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// `tags.key` allows only lowercase ascii alphanumerics, `:` and `-`, so a tag
/// key cannot use [`unique`]'s `_` separator.
fn unique_key(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    profile_id: i64,
    tag_service_id: i64,
    warehouse_id: Uuid,
    catalog_name: String,
}

/// A profile of `engine_type` (`"trino"` with a real tag service, or
/// `"starrocks"`), an engine bound to it, a registry catalog attached to that
/// engine, and a warehouse behind the catalog — the chain every one of these
/// triggers resolves through.
async fn seed(db: &Db, engine_type: &str) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique(&format!("{engine_type}_clrtrig"));
    let (config, tag_service_id) = if engine_type == "trino" {
        let tag_service_name = format!("{profile_name}_tag");
        let tag_service = policy
            .create_service(&tag_service_name, "tag", &serde_json::json!({}))
            .await
            .expect("create tag service");
        (
            serde_json::json!({ "tagService": tag_service_name }),
            tag_service.id,
        )
    } else {
        (serde_json::json!({}), 0)
    };
    let profile = policy
        .create_service(&profile_name, engine_type, &config)
        .await
        .expect("create profile");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("clrtrig-ws"))
            .bind("Clearance Trigger WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-clrtrig','AKIATEST') RETURNING id",
    )
    .bind(unique("clrtrig-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("clrtrig_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,$2,'egret','{}'::jsonb,$3) RETURNING id",
    )
    .bind(unique("clrtrig-eng"))
    .bind(engine_type)
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("clrtrig_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    Fixture {
        profile_id: profile.id,
        tag_service_id,
        warehouse_id,
        catalog_name,
    }
}

async fn new_role_with_holder(db: &Db, label: &str) -> (String, String) {
    let role_name = unique(label);
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&db.pool)
            .await
            .expect("role");
    let username = unique("holder");
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'H',$2) RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(&username)
    .fetch_one(&db.pool)
    .await
    .expect("user");
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(user_id)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");
    assert_eq!(res.rows_affected(), 1, "the role grant row must exist");
    (role_name, username)
}

/// Every clearance policy for `tag_id` under `service_id`, as `(name, body)`.
async fn clearance_policies(db: &Db, service_id: i64, tag_id: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND (name = $2 OR name LIKE $2 || '-%') ORDER BY name",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}"))
    .fetch_all(&db.pool)
    .await
    .expect("policy read")
}

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// **Input 1: the tag's own GATE.** `set_policy` must produce the compiled deny
/// there and then. Nothing here calls a compile: the only thing between the write
/// and the assertion is `set_policy` returning.
#[tokio::test]
async fn setting_a_clearance_compiles_the_deny_with_no_second_write() {
    let Some(db) = connect().await else { return };
    let _permit = SERIAL.acquire().await.expect("permit");
    let f = seed(&db, "trino").await;
    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());

    let tag = tags
        .create(&unique_key("clrtrig-pii"), "Gated", "", "#ff0000", None)
        .await
        .expect("create tag");
    let (role_name, username) = new_role_with_holder(&db, "clrtrig-role").await;

    assert!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .is_empty(),
        "nothing may be compiled before the gate is set, or this test proves nothing"
    );

    pol.set_policy(tag.id, std::slice::from_ref(&role_name), &[], None)
        .await
        .expect("set the gate");

    let compiled = clearance_policies(&db, f.tag_service_id, tag.id).await;
    assert_eq!(
        compiled.len(),
        1,
        "setting the gate must compile its deny IMMEDIATELY — the `classification_policies` row \
         was always written correctly, which is exactly why asserting on it would prove nothing"
    );
    let body = &compiled[0].1;
    assert_eq!(
        body["resources"]["tag"]["values"][0]
            .as_str()
            .expect("tag resource"),
        "Gated",
        "the deny must name the tag the plugin resolves"
    );
    assert_eq!(
        body["denyExceptions"][0]["users"][0]
            .as_str()
            .expect("exception user"),
        username,
        "and except the clearing role's holder by the username the engine sees"
    );

    // **The lifting half, in the same test**, because a clearance that cannot be
    // lifted is the one-way door SP4a had to reopen once: the reap must happen on
    // the `clear_policy` write, not on some later unrelated recompile.
    pol.clear_policy(tag.id).await.expect("lift the gate");
    assert!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .is_empty(),
        "lifting the gate must reap the deny on that write — a deny nobody can lift is worse than \
         one that was never created"
    );
}

/// **Input 2: which columns carry the tag.** SP4a Task 7 already recompiles on a
/// catalog-tag write; on StarRocks a clearance is a per-column expansion, so this
/// verifies the existing trigger now covers clearance rather than assuming it.
#[tokio::test]
async fn tagging_a_column_extends_the_clearance_on_starrocks() {
    let Some(db) = connect().await else { return };
    let _permit = SERIAL.acquire().await.expect("permit");
    let f = seed(&db, "starrocks").await;
    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());
    let catalog_tags = egret_bi::CatalogTagRepo::new(db.clone());

    let tag = tags
        .create(&unique_key("clrtrig-sr"), "SrGated", "", "#ff0000", None)
        .await
        .expect("create tag");
    let (role_name, _username) = new_role_with_holder(&db, "clrtrig-sr-role").await;
    pol.set_policy(tag.id, &[role_name], &[], None)
        .await
        .expect("set the gate");

    assert!(
        clearance_policies(&db, f.profile_id, tag.id)
            .await
            .is_empty(),
        "no column carries the tag yet, so there is nothing to withhold"
    );

    catalog_tags
        .assign_catalog(tag.id, f.warehouse_id, "sales", "orders", "email", None)
        .await
        .expect("tag a column");

    let compiled = clearance_policies(&db, f.profile_id, tag.id).await;
    assert_eq!(
        compiled.len(),
        1,
        "tagging a column must extend the clearance on that write — on StarRocks the clearance IS \
         the per-column enumeration, so a missed recompile leaves the newly classified column \
         readable by everyone"
    );
    assert_eq!(
        compiled[0].1["resources"]["column"]["values"][0]
            .as_str()
            .expect("column resource"),
        "email",
        "and the new deny must address the column that was just tagged"
    );

    catalog_tags
        .unassign_catalog(tag.id, f.warehouse_id, "sales", "orders", "email")
        .await
        .expect("untag the column");
    assert!(
        clearance_policies(&db, f.profile_id, tag.id)
            .await
            .is_empty(),
        "untagging must reap it on that write too — the opposite direction, and the one that \
         leaves a deny on a column nothing classifies any more"
    );
}

/// **A tag RENAME is a write to input 1, and its affected set was blind to
/// clearance.** `affected_services_for_tag_rename` early-returned unless a
/// `perm.rules` row selected the key — and a GATED tag needs no rule. So renaming
/// a gated-but-unruled tag recompiled nothing, the deny kept naming the OLD tag,
/// and at the next tag-map refresh it matched nothing: fail OPEN, triggered by a
/// display rename. There is deliberately NO rule in this test.
#[tokio::test]
async fn renaming_a_gated_tag_recompiles_its_clearance() {
    let Some(db) = connect().await else { return };
    let _permit = SERIAL.acquire().await.expect("permit");
    let f = seed(&db, "trino").await;
    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());

    let tag = tags
        .create(
            &unique_key("clrtrig-ren"),
            "Before Rename",
            "",
            "#ff0000",
            None,
        )
        .await
        .expect("create tag");
    let (role_name, _username) = new_role_with_holder(&db, "clrtrig-ren-role").await;
    pol.set_policy(tag.id, &[role_name], &[], None)
        .await
        .expect("set the gate");
    assert_eq!(
        clearance_policies(&db, f.tag_service_id, tag.id).await[0].1["resources"]["tag"]["values"]
            [0]
        .as_str()
        .expect("tag resource"),
        "Before Rename"
    );

    tags.update(tag.id, Some("After Rename"), None, None)
        .await
        .expect("rename");

    assert_eq!(
        clearance_policies(&db, f.tag_service_id, tag.id).await[0].1["resources"]["tag"]["values"]
            [0]
        .as_str()
        .expect("tag resource"),
        "After Rename",
        "the rename must recompile the clearance — a deny naming a tag that no longer exists \
         matches nothing, so the classified column starts answering queries"
    );
}

/// **Deleting a tag is a write to inputs 1 AND 2**, since both cascade away with
/// the row. A gated tag with no rule used to recompile nothing on delete, leaving
/// its deny served to every plugin with nothing left in the database that would
/// ever sweep it: a clearance nobody could lift.
#[tokio::test]
async fn deleting_a_gated_tag_reaps_its_clearance() {
    let Some(db) = connect().await else { return };
    let _permit = SERIAL.acquire().await.expect("permit");
    let f = seed(&db, "trino").await;
    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());

    let tag = tags
        .create(&unique_key("clrtrig-del"), "Doomed", "", "#ff0000", None)
        .await
        .expect("create tag");
    let (role_name, _username) = new_role_with_holder(&db, "clrtrig-del-role").await;
    pol.set_policy(tag.id, &[role_name], &[], None)
        .await
        .expect("set the gate");
    assert_eq!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .len(),
        1,
        "the gate must compile before deleting the tag can prove anything"
    );

    tags.delete(tag.id).await.expect("delete the tag");

    assert!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .is_empty(),
        "deleting the tag must reap its deny on that write — nothing left in the database would \
         ever produce it again, so nothing would ever sweep it either"
    );
    assert_eq!(
        f.catalog_name, f.catalog_name,
        "fixture field kept for the StarRocks sibling"
    );
}

/// **SP4c review I5 — the clearance author is told what compiled, not just that
/// the row was saved.**
///
/// A gate naming only GROUPS compiles to nothing on any engine: D7 measured that
/// a Ranger item naming a group matches nobody, so a deny emitted for one would
/// withhold the column from everybody, and `egret_perm_compile::clearance` skips
/// it and REPORTS the skip. Until this fix the report stopped at a
/// `tracing::warn!`: `set_policy` returned `()`, the endpoint answered
/// `{"ok": true}`, and the UI showed the clearance as active over a data plane
/// that had never been given one. That is the same "a create that succeeded but
/// compiled nothing" fact the perm mutation responses already carry as
/// `SkipReport`, on the one surface that dropped it.
///
/// Both directions are asserted against ONE fixture, because a test that only
/// checked the group-only case is also passed by an implementation that reports
/// every write as skipped.
#[tokio::test]
async fn a_group_only_gate_reports_its_skip_to_the_author() {
    let Some(db) = connect().await else { return };
    let _permit = SERIAL.acquire().await.expect("permit");
    let f = seed(&db, "trino").await;
    let tags = TagRepo::new(db.clone());
    let pol = ClassificationPolicyRepo::new(db.clone(), EventBus::new());

    let tag = tags
        .create(&unique_key("clrskip-pii"), "GatedSkip", "", "#ff0000", None)
        .await
        .expect("create tag");
    let (role_name, _username) = new_role_with_holder(&db, "clrskip-role").await;

    // A ROLE gate compiles, so it reports NO skip — the discriminating half.
    let skipped = pol
        .set_policy(tag.id, &[role_name], &[], None)
        .await
        .expect("set a role gate");
    assert!(
        skipped.iter().all(|s| s.id != tag.id),
        "a role gate this profile CAN express must not be reported as skipped: {skipped:?}"
    );
    assert_eq!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .len(),
        1,
        "sanity: the role gate really did compile"
    );

    // The same tag, re-gated on a GROUP alone. Nothing can be compiled for it,
    // and the author must be told so on the write itself.
    let group_id = Uuid::new_v4();
    let skipped = pol
        .set_policy(tag.id, &[], &[group_id], None)
        .await
        .expect("set a group-only gate");
    let mine: Vec<_> = skipped.iter().filter(|s| s.id == tag.id).collect();
    assert_eq!(
        mine.len(),
        1,
        "a group-only gate compiles to nothing, and the write must SAY so rather than answering \
         ok over a clearance no engine will ever enforce: {skipped:?}"
    );
    assert!(
        matches!(
            mine[0].source_kind,
            egret_perm_compile::SourceKind::Clearance
        ),
        "the skip is a CLEARANCE skip, and `id` is the tags.id: {mine:?}"
    );
    assert!(
        mine[0].reason.contains("required_groups"),
        "the reason must name what could not be expressed, not merely that something was: {mine:?}"
    );
    assert!(
        clearance_policies(&db, f.tag_service_id, tag.id)
            .await
            .is_empty(),
        "and the earlier role gate's deny must be gone — the gate no longer names a role"
    );
}
