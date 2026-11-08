//! Governance SP2b Task 16: `POST /perm/explain` answers a profile on OPA
//! through the IR evaluator (`egret-perm-eval`), over the refresher's
//! snapshot when it is current and a snapshot built in explain's own
//! read-only transaction otherwise. A Ranger profile answers as before, plus
//! `mechanism: "ranger"`.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --features harness \
//!     --test explain_opa
include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::{Action, Effect, PermStore, Selector};
use egret_perm_compile::opa::cache::{Lookup, SnapshotCache};
use egret_perm_compile::{perm_router, PermState};
use egret_perm_eval::{Snapshot, SnapshotInput};
use egret_policy::PolicyStore;
use fixtures::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

static SEEDED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    SEEDED
        .get_or_init(|| async {
            db.migrate().await.expect("migrate");
            seed_service_defs(&db).await;
        })
        .await;
    db
}

fn prefix(name: &str) -> String {
    format!("{name}{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// A router over `db` with `cache` as `PermState.opa_cache`, and a
/// platform-admin session for it.
async fn app(db: &Db, cache: Option<Arc<SnapshotCache>>) -> (axum::Router, String) {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, $1) RETURNING id",
    )
    .bind(format!("xo_admin_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("admin");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("platform_admin");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("pdp"),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(db.pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
        opa_cache: cache,
    });
    (app, token)
}

async fn explain(
    app: &axum::Router,
    token: &str,
    user: &str,
    action: &str,
    target: Value,
) -> (StatusCode, Value, String) {
    let body = json!({ "subject": { "user": user }, "action": action, "target": target });
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/perm/explain")
                .header("Cookie", format!("egret_session={token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "_raw": raw }));
    (status, json, raw)
}

fn target(cat: Uuid, table: Option<&str>, column: Option<&str>) -> Value {
    json!({
        "catalog_id": cat,
        "schema": "s",
        "table": table,
        "column": column,
    })
}

/// A Trino profile with a tag service, catalog `lake` on an engine bound to
/// it, `ann` and `bob` granted Read (and `ann` Write) on the catalog, and
/// classification `pii` on `s.t.ssn` cleared for `bob`'s role. `carol` holds
/// nothing. Committed: explain reads in a transaction of its own.
struct Store {
    profile: i64,
    cat: Uuid,
    pii_name: String,
    ann: String,
    bob: String,
    carol: String,
}

async fn store(db: &Db, prefix: &str, extra: Extra) -> Store {
    let mut names = Names::default();
    let (profile, tag_service, _) = fixtures::profile(db, prefix, "trino", true, &mut names).await;
    assert!(tag_service.is_some(), "the profile has a tag service");
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, prefix, &mut names).await;
    // Pinned to Trino 483, so the Ranger path can name its plugin version.
    let engine = engine_bound_spec(
        &mut tx,
        &format!("{prefix}.engine"),
        profile,
        "trino",
        json!({"imageVersion": "483", "trinoConfig": {"delegationMode": "open"}}),
        &mut names,
    )
    .await;
    let cat = catalog(
        &mut tx,
        &format!("{prefix}.lake"),
        wh,
        &[engine],
        &mut names,
    )
    .await;
    let ann = format!("{prefix}_ann");
    let bob = format!("{prefix}_bob");
    let carol = format!("{prefix}_carol");
    user(&mut tx, &format!("{prefix}.ann"), Some(&ann), &mut names).await;
    let bob_id = user(&mut tx, &format!("{prefix}.bob"), Some(&bob), &mut names).await;
    user(
        &mut tx,
        &format!("{prefix}.carol"),
        Some(&carol),
        &mut names,
    )
    .await;
    grant(
        &mut tx,
        &format!("{prefix}.read"),
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[user_ref(&ann), user_ref(&bob)], &[]),
        &mut names,
    )
    .await;
    grant(
        &mut tx,
        &format!("{prefix}.write"),
        Effect::Allow,
        &[Action::Write],
        scope(cat, None, None, None),
        set(&[user_ref(&ann)], &[]),
        &mut names,
    )
    .await;
    let pii_name = format!("{prefix}_pii");
    let pii = tag(&mut tx, &pii_name, &mut names).await;
    tag_column(&mut tx, pii, wh, "s", "t", "ssn").await;
    let cleared = format!("{prefix}_cleared_role");
    platform_role(&mut tx, &cleared, &mut names).await;
    role_held(&mut tx, bob_id, &cleared).await;
    gate(&mut tx, pii, &[cleared.as_str()], &[]).await;
    match extra {
        Extra::None => {}
        Extra::PublicWrite => {
            grant(
                &mut tx,
                &format!("{prefix}.public_write"),
                Effect::Allow,
                &[Action::Write],
                scope(cat, None, None, None),
                set(&[group_ref("public")], &[]),
                &mut names,
            )
            .await;
        }
        Extra::MaskAndFilter => {
            rule(
                &mut tx,
                &format!("{prefix}.mask"),
                mask("MASK_HASH", None),
                Selector::Path(scope(cat, Some("s"), Some("t"), Some("email"))),
                set(&[user_ref(&bob)], &[]),
                &mut names,
            )
            .await;
            rule(
                &mut tx,
                &format!("{prefix}.filter"),
                row_filter("region = 'eu'"),
                Selector::Path(scope(cat, Some("s"), Some("t"), None)),
                set(&[user_ref(&bob)], &[]),
                &mut names,
            )
            .await;
        }
        Extra::TwoMasks => {
            // Two distinct masks on one column for everyone: the write path
            // refuses this overlap (`overlap.rs`), a stored row from before
            // that check does not, and the evaluator answers `TwoMasks`.
            for (role, mask_type) in [("hash", "MASK_HASH"), ("last4", "MASK_SHOW_LAST_4")] {
                rule(
                    &mut tx,
                    &format!("{prefix}.{role}"),
                    mask(mask_type, None),
                    Selector::Path(scope(cat, Some("s"), Some("t"), Some("email"))),
                    set(&[], &[]),
                    &mut names,
                )
                .await;
            }
        }
        Extra::UnusableGrant => {
            // Written past the authoring check (J-F3), as a row stored before
            // it would be: the lowering refuses it with `UnusableUserRef`,
            // whose message names the ref.
            grant(
                &mut tx,
                &format!("{prefix}.unusable"),
                Effect::Allow,
                &[Action::Read],
                scope(cat, None, None, None),
                set(&[user_ref(LEAKED_REF)], &[]),
                &mut names,
            )
            .await;
        }
    }
    tx.commit().await.expect("commit");
    Store {
        profile,
        cat,
        pii_name,
        ann,
        bob,
        carol,
    }
}

const LEAKED_REF: &str = "someone.else@private.example";

#[derive(Clone, Copy)]
enum Extra {
    None,
    PublicWrite,
    MaskAndFilter,
    UnusableGrant,
    TwoMasks,
}

async fn set_mechanism(db: &Db, profile: i64, mechanism: &str) {
    sqlx::query("UPDATE policy.authz_services SET mechanism = $2 WHERE id = $1")
        .bind(profile)
        .bind(mechanism)
        .execute(&db.pool)
        .await
        .expect("mechanism");
}

async fn key(db: &Db, profile: i64) -> (i64, i64) {
    sqlx::query_as("SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1")
        .bind(profile)
        .fetch_one(&db.pool)
        .await
        .expect("key")
}

/// The single profile of an explain response.
fn only_profile(body: &Value) -> &Value {
    let profiles = body["profiles"].as_array().expect("profiles");
    assert_eq!(profiles.len(), 1, "one profile serves the catalog: {body}");
    &profiles[0]
}

fn access<'a>(profile: &'a Value, access_type: &str) -> &'a Value {
    profile["accesses"]
        .as_array()
        .expect("accesses")
        .iter()
        .find(|a| a["access_type"] == access_type)
        .unwrap_or_else(|| panic!("no {access_type} access: {profile}"))
}

fn line_outcomes(profile: &Value) -> Vec<(String, String)> {
    profile["effect_lines"]
        .as_array()
        .expect("effect lines")
        .iter()
        .map(|l| {
            (
                l["about"].as_str().unwrap().to_string(),
                l["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_opa_profile_is_explained_by_the_evaluator_with_its_reason() {
    // Arrange
    let db = db().await;
    let st = store(&db, &prefix("xo1"), Extra::None).await;
    set_mechanism(&db, st.profile, "opa").await;
    let (app, token) = app(&db, None).await;
    let ssn = target(st.cat, Some("t"), Some("ssn"));

    // Act
    let (s_ann, ann, _) = explain(&app, &token, &st.ann, "read", ssn.clone()).await;
    let (s_bob, bob, _) = explain(&app, &token, &st.bob, "read", ssn).await;
    // An unclassified column, so no withhold reaches carol.
    let (s_carol, carol, _) = explain(
        &app,
        &token,
        &st.carol,
        "read",
        target(st.cat, Some("t"), Some("id")),
    )
    .await;
    let (s_write, write, _) = explain(
        &app,
        &token,
        &st.ann,
        "write",
        target(st.cat, Some("t"), None),
    )
    .await;
    let (s_cw, carol_write, _) = explain(
        &app,
        &token,
        &st.carol,
        "write",
        target(st.cat, Some("t"), None),
    )
    .await;

    // Assert
    for (s, b) in [
        (s_ann, &ann),
        (s_bob, &bob),
        (s_carol, &carol),
        (s_write, &write),
        (s_cw, &carol_write),
    ] {
        assert_eq!(s, StatusCode::OK, "{b}");
    }
    // ann: the withhold on pii refuses her, explicitly.
    let p = only_profile(&ann);
    assert_eq!(p["mechanism"], "opa", "{p}");
    let a = access(p, "select");
    assert_eq!(a["decision"]["kind"], "denied", "{a}");
    assert_eq!(a["decision"]["reason"], "deny_policy_item", "{a}");
    assert_eq!(a["decision"]["policy_id"], Value::Null, "{a}");
    assert_eq!(a["attribution"], Value::Null, "{a}");
    let reason = a["ir_reason"].as_str().expect("ir_reason");
    assert!(reason.contains(&st.pii_name), "{reason}");
    assert_eq!(
        p["effect_lines"][0]["text"],
        format!("{} is not allowed to select s.t.ssn: {reason}", st.ann),
        "{p}"
    );
    assert_eq!(
        line_outcomes(p),
        vec![("access".to_string(), "refused".to_string())],
        "a refused read has no column or rows line: {p}"
    );
    // bob: cleared, and granted.
    let p = only_profile(&bob);
    assert_eq!(p["mechanism"], "opa", "{p}");
    let a = access(p, "select");
    assert_eq!(a["decision"]["kind"], "allowed", "{a}");
    let by = a["decision"]["policy_name"].as_str().expect("policy_name");
    assert!(by.starts_with("grant "), "{a}");
    assert_eq!(a["ir_reason"], by, "{a}");
    assert_eq!(
        p["effect_lines"][0]["text"],
        format!("{} is allowed to select s.t.ssn: {by}", st.bob),
        "{p}"
    );
    // carol: nothing allows her, and nothing refused her.
    let a = access(only_profile(&carol), "select");
    assert_eq!(a["decision"]["kind"], "denied", "{a}");
    assert_eq!(a["decision"]["reason"], "no_policy_matched", "{a}");
    // write: insert and delete, each its own verdict.
    for (body, kind) in [(&write, "allowed"), (&carol_write, "denied")] {
        let p = only_profile(body);
        let types: Vec<&str> = p["accesses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["access_type"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["insert", "delete"], "{p}");
        for t in types {
            assert_eq!(access(p, t)["decision"]["kind"], kind, "{p}");
        }
        assert!(p.get("opa").is_none(), "a write has no values: {p}");
    }
}

#[tokio::test]
async fn an_opa_profile_reports_its_mask_and_row_filter() {
    // Arrange
    let db = db().await;
    let st = store(&db, &prefix("xo2"), Extra::MaskAndFilter).await;
    set_mechanism(&db, st.profile, "opa").await;
    let (app, token) = app(&db, None).await;
    let email = target(st.cat, Some("t"), Some("email"));

    // Act
    let (_, ann, _) = explain(&app, &token, &st.ann, "read", email.clone()).await;
    let (_, bob, _) = explain(&app, &token, &st.bob, "read", email).await;
    let (_, ann_table, _) = explain(
        &app,
        &token,
        &st.ann,
        "read",
        target(st.cat, Some("t"), None),
    )
    .await;
    let (_, ann_schema, _) =
        explain(&app, &token, &st.ann, "read", target(st.cat, None, None)).await;

    // Assert
    let p = only_profile(&ann);
    let expr = egret_perm_eval::render_mask("MASK_HASH", None, "email", "varchar").unwrap();
    assert_eq!(
        p["opa"],
        json!({
            "column_mask": expr,
            "column_mask_type": "MASK_HASH",
            "row_filter": "(region = 'eu')",
        }),
        "{p}"
    );
    assert_eq!(
        line_outcomes(p),
        vec![
            ("access".to_string(), "allowed".to_string()),
            ("column".to_string(), "masked".to_string()),
            ("rows".to_string(), "filtered".to_string()),
        ],
        "{p}"
    );
    assert_eq!(
        p["effect_lines"][1]["text"],
        format!("{} sees email masked by MASK_HASH: {expr}", st.ann),
        "{p}"
    );
    assert_eq!(
        p["effect_lines"][2]["text"],
        format!("{} sees only the rows of t where (region = 'eu')", st.ann),
        "{p}"
    );
    for key in ["mask", "row_filter", "other_columns_access"] {
        assert!(p.get(key).is_none(), "no Ranger {key} on OPA: {p}");
    }
    // bob is exempt from both.
    let p = only_profile(&bob);
    assert_eq!(
        p["opa"],
        json!({ "column_mask": null, "column_mask_type": null, "row_filter": null }),
        "{p}"
    );
    assert_eq!(
        line_outcomes(p),
        vec![
            ("access".to_string(), "allowed".to_string()),
            ("column".to_string(), "real_value".to_string()),
            ("rows".to_string(), "all_rows".to_string()),
        ],
        "{p}"
    );
    // A table: the filter, no column.
    let p = only_profile(&ann_table);
    assert_eq!(
        p["opa"],
        json!({ "column_mask": null, "column_mask_type": null, "row_filter": "(region = 'eu')" }),
        "{p}"
    );
    assert_eq!(
        line_outcomes(p),
        vec![
            ("access".to_string(), "allowed".to_string()),
            ("rows".to_string(), "filtered".to_string()),
        ],
        "{p}"
    );
    // A schema: no values to report.
    let p = only_profile(&ann_schema);
    assert!(p.get("opa").is_none(), "{p}");
}

#[tokio::test]
async fn a_ranger_profile_answers_exactly_as_before_plus_mechanism_ranger() {
    // Arrange: the profile stays on Ranger, compiled so its download decides.
    let db = db().await;
    let st = store(&db, &prefix("xo3"), Extra::MaskAndFilter).await;
    egret_perm_compile::recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        st.profile,
    )
    .await
    .expect("compile");
    let (app, token) = app(&db, None).await;

    // Act
    let (status, body, _) = explain(
        &app,
        &token,
        &st.ann,
        "read",
        target(st.cat, Some("t"), Some("email")),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::OK, "{body}");
    let p = only_profile(&body);
    assert_eq!(p["mechanism"], "ranger", "{p}");
    // Exactly the keys a Ranger profile carried before Task 16, plus
    // `mechanism`: no `opa`, and no `ir_reason` on any verdict.
    let keys: BTreeSet<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
    let before: BTreeSet<&str> = [
        "service_id",
        "service_name",
        "engine",
        "accesses",
        "tags",
        "tag_policies_in_download",
        "mask",
        "row_filter",
        "effect_lines",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        keys.difference(&before).copied().collect::<Vec<_>>(),
        ["mechanism"],
        "{p}"
    );
    assert_eq!(before.difference(&keys).count(), 0, "{p}");
    let a = access(p, "select");
    let mut want_keys = vec!["access_type", "attribution", "decision"];
    want_keys.sort();
    let mut got: Vec<&str> = a.as_object().unwrap().keys().map(String::as_str).collect();
    got.sort();
    assert_eq!(got, want_keys, "{a}");
    // Decided by egret-ranger-eval over the compiled download, as before.
    assert_eq!(a["decision"]["kind"], "allowed", "{a}");
    assert!(
        a["decision"]["policy_id"].is_i64(),
        "a Ranger verdict names its policy id: {a}"
    );
    assert_eq!(a["attribution"]["kind"], "grant", "{a}");
    assert_eq!(p["mask"]["decision"]["kind"], "masked", "{p}");
    assert_eq!(p["row_filter"]["decision"]["kind"], "filtered", "{p}");
}

#[tokio::test]
async fn a_profile_whose_snapshot_cannot_be_built_is_refused_without_its_error() {
    // Arrange
    let db = db().await;
    let st = store(&db, &prefix("xo4"), Extra::UnusableGrant).await;
    set_mechanism(&db, st.profile, "opa").await;
    let (app, token) = app(&db, None).await;

    // Act
    let (s_read, read, raw_read) = explain(
        &app,
        &token,
        &st.bob,
        "read",
        target(st.cat, Some("t"), Some("id")),
    )
    .await;
    let (s_write, write, raw_write) = explain(
        &app,
        &token,
        &st.ann,
        "write",
        target(st.cat, Some("t"), None),
    )
    .await;

    // Assert
    assert_eq!(s_read, StatusCode::OK, "{read}");
    assert_eq!(s_write, StatusCode::OK, "{write}");
    for (body, raw, n) in [(&read, &raw_read, 1), (&write, &raw_write, 2)] {
        assert!(
            !raw.contains(LEAKED_REF),
            "the build error is not echoed: {raw}"
        );
        assert!(!raw.contains("private.example"), "{raw}");
        let p = only_profile(body);
        assert_eq!(p["mechanism"], "opa", "{p}");
        assert!(p.get("opa").is_none(), "{p}");
        let accesses = p["accesses"].as_array().unwrap();
        assert_eq!(accesses.len(), n, "{p}");
        for a in accesses {
            assert_eq!(a["decision"]["kind"], "denied", "{a}");
            assert_eq!(a["decision"]["reason"], "no_policy_matched", "{a}");
            assert_eq!(
                a["ir_reason"], "no OPA snapshot: build failed (UnusableUserRef)",
                "{a}"
            );
        }
        assert!(line_outcomes(p).iter().all(|(_, o)| o == "refused"), "{p}");
    }
}

#[tokio::test]
async fn the_cached_snapshot_is_used_only_at_the_profiles_current_key() {
    // Arrange: a cache holding an EMPTY snapshot for the profile — every
    // access it answers is denied — so which snapshot explain used shows.
    let db = db().await;
    let st = store(&db, &prefix("xo5"), Extra::None).await;
    set_mechanism(&db, st.profile, "opa").await;
    let current = key(&db, st.profile).await;
    let empty = |key| {
        Snapshot::build(SnapshotInput {
            service_id: st.profile,
            key,
            statements: Vec::new(),
            classified: Vec::new(),
            lineage_principal: "egret-lineage".into(),
        })
        .expect("empty snapshot")
    };
    let at_key = Arc::new(SnapshotCache::default());
    at_key.insert_for_test(st.profile, empty(current));
    // Stale on `policy_version` only, and on `tag_version` only: a comparison
    // of either half alone uses one of them.
    let stale_policy = Arc::new(SnapshotCache::default());
    stale_policy.insert_for_test(st.profile, empty((current.0 - 1, current.1)));
    let stale_tag = Arc::new(SnapshotCache::default());
    stale_tag.insert_for_test(st.profile, empty((current.0, current.1 - 1)));
    let (app_at_key, token) = app(&db, Some(at_key)).await;
    let (app_stale_policy, _) = app(&db, Some(stale_policy)).await;
    let (app_stale_tag, _) = app(&db, Some(stale_tag)).await;
    let id = target(st.cat, Some("t"), Some("id"));

    // Act
    let (_, from_cache, _) = explain(&app_at_key, &token, &st.bob, "read", id.clone()).await;
    let (_, rebuilt_policy, _) =
        explain(&app_stale_policy, &token, &st.bob, "read", id.clone()).await;
    let (_, rebuilt_tag, _) = explain(&app_stale_tag, &token, &st.bob, "read", id).await;

    // Assert
    let a = access(only_profile(&from_cache), "select");
    assert_eq!(
        a["decision"]["kind"], "denied",
        "the cached snapshot at the current key answered: {a}"
    );
    for rebuilt in [&rebuilt_policy, &rebuilt_tag] {
        let a = access(only_profile(rebuilt), "select");
        assert_eq!(
            a["decision"]["kind"], "allowed",
            "a stale cached snapshot is not used; explain built a fresh one: {a}"
        );
        assert!(
            !a["ir_reason"]
                .as_str()
                .unwrap()
                .contains("previous snapshot"),
            "a fresh build is not a splice: {a}"
        );
    }
}

#[tokio::test]
async fn an_evaluator_error_refuses_that_profile_and_not_the_request() {
    // Arrange
    let db = db().await;
    let st = store(&db, &prefix("xo6"), Extra::TwoMasks).await;
    set_mechanism(&db, st.profile, "opa").await;
    // Settles review ⚠️ 1: the refresher BUILDS this state (two stored mask
    // RULES are not a stored-policy overlap, and nothing here is an operator
    // row), so the surface answers it and the error is the evaluator's.
    let cache = Arc::new(SnapshotCache::default());
    cache.refresh_once(&db.pool).await.expect("refresh");
    assert!(
        matches!(cache.get(st.profile), Lookup::Ready(_)),
        "the refresher builds the two-mask state: {:?}",
        cache.get(st.profile)
    );
    let (app, token) = app(&db, Some(cache)).await;

    // Act
    let (s_email, email, raw) = explain(
        &app,
        &token,
        &st.ann,
        "read",
        target(st.cat, Some("t"), Some("email")),
    )
    .await;
    // The same user on a column no mask reaches: the snapshot itself is fine.
    let (s_id, id, _) = explain(
        &app,
        &token,
        &st.ann,
        "read",
        target(st.cat, Some("t"), Some("id")),
    )
    .await;

    // Assert
    assert_eq!(s_email, StatusCode::OK, "{email}");
    assert_eq!(s_id, StatusCode::OK, "{id}");
    let p = only_profile(&email);
    assert_eq!(p["mechanism"], "opa", "{p}");
    assert!(
        p.get("opa").is_none(),
        "no values beside an evaluator error: {p}"
    );
    let a = access(p, "select");
    assert_eq!(a["decision"]["kind"], "denied", "{a}");
    assert_eq!(a["decision"]["reason"], "no_policy_matched", "{a}");
    assert_eq!(
        a["ir_reason"], "no OPA answer: evaluator error (TwoMasks)",
        "{a}"
    );
    assert_eq!(
        line_outcomes(p),
        vec![("access".to_string(), "refused".to_string())],
        "{p}"
    );
    for leaked in ["MASK_HASH", "MASK_SHOW_LAST_4", "sha256", "two masks"] {
        assert!(
            !raw.contains(leaked),
            "the error is not echoed ({leaked}): {raw}"
        );
    }
    let a = access(only_profile(&id), "select");
    assert_eq!(a["decision"]["kind"], "allowed", "{a}");
}

/// A hand-written (`managed_by = 'operator'`) allow on the profile: a row the
/// evaluator would ignore, so the refresher refuses the profile (C8).
async fn operator_policy(db: &Db, profile: i64, prefix: &str, user: &str) -> i64 {
    let policy = PolicyStore::new(db.clone());
    let svc = policy.get_service_by_id(profile).await.expect("profile");
    let name = format!("{prefix}-operator");
    let any = json!({ "values": ["*"], "isExcludes": false, "isRecursive": false });
    policy
        .create_policy(
            profile,
            &name,
            &json!({
                "name": name,
                "service": svc.name,
                "serviceType": "trino",
                "policyType": 0,
                "isEnabled": true,
                "resources": { "catalog": any, "schema": any, "table": any, "column": any },
                "policyItems": [{
                    "users": [user], "groups": [], "roles": [],
                    "accesses": [{ "type": "select", "isAllowed": true }],
                    "delegateAdmin": false,
                }],
                "denyPolicyItems": [],
                "allowExceptions": [],
                "denyExceptions": [],
            }),
        )
        .await
        .expect("operator policy")
}

fn assert_refused(body: &Value, class: &str) {
    let p = only_profile(body);
    assert_eq!(p["mechanism"], "opa", "{p}");
    assert!(p.get("opa").is_none(), "{p}");
    for a in p["accesses"].as_array().unwrap() {
        assert_eq!(a["decision"]["kind"], "denied", "{a}");
        assert_eq!(a["decision"]["reason"], "no_policy_matched", "{a}");
        assert_eq!(
            a["ir_reason"],
            format!("no OPA snapshot: build failed ({class})"),
            "{a}"
        );
    }
}

#[tokio::test]
async fn a_stored_state_refusal_refuses_the_profile_while_the_cache_is_empty() {
    // Arrange: bob is granted Read, and an operator row sits on the profile.
    let db = db().await;
    let p = prefix("xo7");
    let st = store(&db, &p, Extra::None).await;
    operator_policy(&db, st.profile, &p, &st.carol).await;
    set_mechanism(&db, st.profile, "opa").await;
    let refresher = SnapshotCache::default();
    refresher.refresh_once(&db.pool).await.expect("refresh");
    let (no_cache, token) = app(&db, None).await;
    let (empty_cache, _) = app(&db, Some(Arc::new(SnapshotCache::default()))).await;
    let id = target(st.cat, Some("t"), Some("id"));

    // Act
    let (s1, uncached, _) = explain(&no_cache, &token, &st.bob, "read", id.clone()).await;
    let (s2, empty, _) = explain(&empty_cache, &token, &st.bob, "read", id).await;

    // Assert: the surface refuses everything on this profile, and so does
    // explain — not bob's grant.
    assert!(
        matches!(refresher.get(st.profile), Lookup::NotBuilt { .. }),
        "the refresher refuses the stored state"
    );
    assert_eq!(s1, StatusCode::OK, "{uncached}");
    assert_eq!(s2, StatusCode::OK, "{empty}");
    assert_refused(&uncached, "StoredStateRefused");
    assert_refused(&empty, "StoredStateRefused");
}

#[tokio::test]
async fn a_not_built_cache_entry_at_the_current_key_refuses_the_profile() {
    // Arrange: the refresher refuses the profile at key K; the operator row
    // is then removed without moving K, so a build of its own would succeed.
    let db = db().await;
    let p = prefix("xo8");
    let st = store(&db, &p, Extra::None).await;
    let policy_id = operator_policy(&db, st.profile, &p, &st.carol).await;
    set_mechanism(&db, st.profile, "opa").await;
    let cache = Arc::new(SnapshotCache::default());
    cache.refresh_once(&db.pool).await.expect("refresh");
    assert!(matches!(cache.get(st.profile), Lookup::NotBuilt { .. }));
    let k = key(&db, st.profile).await;
    sqlx::query("DELETE FROM policy.authz_policies WHERE id = $1")
        .bind(policy_id)
        .execute(&db.pool)
        .await
        .expect("delete operator row");
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = $2, tag_version = $3 WHERE id = $1",
    )
    .bind(st.profile)
    .bind(k.0)
    .bind(k.1)
    .execute(&db.pool)
    .await
    .expect("hold the key");
    let (cached, token) = app(&db, Some(cache)).await;
    let (uncached, _) = app(&db, None).await;
    let id = target(st.cat, Some("t"), Some("id"));

    // Act
    let (_, from_cache, _) = explain(&cached, &token, &st.bob, "read", id.clone()).await;
    let (_, built, _) = explain(&uncached, &token, &st.bob, "read", id).await;

    // Assert: what the surface holds at K decides, not a build of explain's own.
    assert_refused(&from_cache, "StoredStateRefused");
    assert_eq!(
        access(only_profile(&built), "select")["decision"]["kind"],
        "allowed",
        "the state itself builds now: {built}"
    );
}

#[tokio::test]
async fn a_failed_data_lowering_is_answered_from_the_previous_snapshot_as_the_surface_does() {
    // Arrange: the refresher holds a good snapshot at K1; then a stored grant
    // the lowering refuses lands and the key moves to K2.
    let db = db().await;
    let p = prefix("xo9");
    let st = store(&db, &p, Extra::None).await;
    set_mechanism(&db, st.profile, "opa").await;
    let cache = Arc::new(SnapshotCache::default());
    cache.refresh_once(&db.pool).await.expect("refresh");
    assert!(matches!(cache.get(st.profile), Lookup::Ready(_)));
    {
        let mut names = Names::default();
        let mut tx = db.pool.begin().await.expect("tx");
        grant(
            &mut tx,
            &format!("{p}.unusable"),
            Effect::Allow,
            &[Action::Read],
            scope(st.cat, None, None, None),
            set(&[user_ref(LEAKED_REF)], &[]),
            &mut names,
        )
        .await;
        sqlx::query(
            "UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1",
        )
        .bind(st.profile)
        .execute(&mut *tx)
        .await
        .expect("bump");
        tx.commit().await.expect("commit");
    }
    let (cached, token) = app(&db, Some(Arc::clone(&cache))).await;
    let (uncached, _) = app(&db, None).await;
    let id = target(st.cat, Some("t"), Some("id"));

    // Act: before the refresher has seen K2, then after it spliced.
    let (_, before_refresh, raw) = explain(&cached, &token, &st.bob, "read", id.clone()).await;
    cache.refresh_once(&db.pool).await.expect("refresh");
    let (_, after_refresh, _) = explain(&cached, &token, &st.bob, "read", id.clone()).await;
    let (_, no_previous, _) = explain(&uncached, &token, &st.bob, "read", id).await;

    // Assert
    assert!(
        matches!(cache.get(st.profile), Lookup::Ready(_)),
        "the refresher spliced"
    );
    for body in [&before_refresh, &after_refresh] {
        let a = access(only_profile(body), "select");
        assert_eq!(
            a["decision"]["kind"], "allowed",
            "the surface answers bob from the previous data: {a}"
        );
        let reason = a["ir_reason"].as_str().unwrap();
        assert!(reason.starts_with("grant "), "{reason}");
        assert!(reason.contains("from the previous snapshot"), "{reason}");
    }
    assert!(!raw.contains(LEAKED_REF), "{raw}");
    // With no previous snapshot there is nothing to splice into.
    assert_refused(&no_previous, "UnusableUserRef");
}

/// Task 18 fix round 1 (I2): explain answers the reserved lineage principal
/// as the OPA surface does. A `public` write grant lets carol write but not
/// egret-lineage, whose insert and delete are refused naming the lineage
/// rule; its read follows the grants, as anyone's.
#[tokio::test]
async fn explain_refuses_the_lineage_principal_a_public_write_as_the_surface_does() {
    // Arrange
    let db = db().await;
    let st = store(&db, &prefix("xo10"), Extra::PublicWrite).await;
    set_mechanism(&db, st.profile, "opa").await;
    let (app, token) = app(&db, None).await;
    let lineage = egret_core::engine_principals::LINEAGE_PRINCIPAL;
    let t = target(st.cat, Some("t"), None);

    // Act
    let (s_lp, lp, _) = explain(&app, &token, lineage, "write", t.clone()).await;
    let (s_carol, carol, _) = explain(&app, &token, &st.carol, "write", t).await;

    // Assert
    assert_eq!(s_lp, StatusCode::OK, "{lp}");
    assert_eq!(s_carol, StatusCode::OK, "{carol}");
    for access_type in ["insert", "delete"] {
        let a = access(only_profile(&lp), access_type);
        assert_eq!(a["decision"]["kind"], "denied", "{a}");
        let reason = a["ir_reason"].as_str().expect("ir_reason");
        assert!(
            reason.contains("lineage principal") && reason.contains(lineage),
            "{reason}"
        );
        let a = access(only_profile(&carol), access_type);
        assert_eq!(a["decision"]["kind"], "allowed", "{a}");
    }
}

/// SP2b Task 20F (final review T16 M1): explain follows the profile's
/// mechanism, so it says which bound engines still run the other one.
#[tokio::test]
async fn an_engine_not_yet_reapplied_to_the_profiles_mechanism_is_named() {
    // Arrange: an OPA profile whose bound engine is recorded on Ranger.
    let db = db().await;
    let st = store(&db, &prefix("xo11"), Extra::None).await;
    set_mechanism(&db, st.profile, "opa").await;
    let set_applied = |mechanism: Option<&'static str>| {
        let pool = db.pool.clone();
        let profile = st.profile;
        async move {
            sqlx::query(
                "UPDATE engine_mgmt.engines SET applied_mechanism = $2 \
                 WHERE authz_service_id = $1",
            )
            .bind(profile)
            .bind(mechanism)
            .execute(&pool)
            .await
            .expect("applied_mechanism");
        }
    };
    let engine: String =
        sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(st.profile)
            .fetch_one(&db.pool)
            .await
            .expect("the bound engine");
    let (app, token) = app(&db, None).await;
    let col = || target(st.cat, Some("t"), Some("id"));

    // Act + Assert: on Ranger while the profile is on OPA, it is named.
    set_applied(Some("ranger")).await;
    let (s, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(
        only_profile(&body)["engines_not_reapplied"],
        json!([engine]),
        "{body}"
    );

    // Re-applied: nothing to say.
    set_applied(Some("opa")).await;
    let (_, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert!(
        only_profile(&body).get("engines_not_reapplied").is_none(),
        "{body}"
    );

    // No record (never applied, or external): not named either.
    set_applied(None).await;
    let (_, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert!(
        only_profile(&body).get("engines_not_reapplied").is_none(),
        "{body}"
    );

    // And back on Ranger with the engine still recorded on OPA: named on the
    // Ranger profile too.
    set_mechanism(&db, st.profile, "ranger").await;
    set_applied(Some("opa")).await;
    let (s, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let p = only_profile(&body);
    assert_eq!(p["mechanism"], "ranger", "{p}");
    assert_eq!(p["engines_not_reapplied"], json!([engine]), "{p}");

    // Fix round 1: an engine a re-sent switch would not re-apply is never
    // named, as `reapply_bound_engines` skips it: one being deleted ...
    let set_engine = |sql: &'static str| {
        let pool = db.pool.clone();
        let profile = st.profile;
        async move {
            sqlx::query(sql)
                .bind(profile)
                .execute(&pool)
                .await
                .expect("engine row");
        }
    };
    set_engine("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE authz_service_id = $1")
        .await;
    let (_, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert!(
        only_profile(&body).get("engines_not_reapplied").is_none(),
        "deleting: {body}"
    );
    // ... and an external one.
    set_engine(
        "UPDATE engine_mgmt.engines SET status = 'running', origin = 'external', \
                external_host = 'trino.example', external_port = 8080, \
                ranger_plugin_version = '2.5.0', namespace = '', spec = '{}'::jsonb \
         WHERE authz_service_id = $1",
    )
    .await;
    // On OPA (the Ranger path would judge the external engine's plugin), with
    // the record on the other mechanism.
    set_mechanism(&db, st.profile, "opa").await;
    set_applied(Some("ranger")).await;
    let (_, body, _) = explain(&app, &token, &st.bob, "read", col()).await;
    assert!(
        only_profile(&body).get("engines_not_reapplied").is_none(),
        "external: {body}"
    );
}

const NO_TAG_WARNING: &str = "This profile has no tag service, so classification rules and \
                              clearances do not apply to its engines. Bind a tag service in the \
                              profile's settings.";

/// Explains `read` of a column on a Trino profile that is on `mechanism`,
/// with or without its tag service, and returns the profile's answer.
async fn explain_with_tag_service(mechanism: &str, tagged: bool) -> Value {
    let db = db().await;
    let st = store(&db, &prefix("xo_nt"), Extra::None).await;
    if !tagged {
        sqlx::query(
            "UPDATE policy.authz_services SET config_jsonb = config_jsonb - 'tagService' \
              WHERE id = $1",
        )
        .bind(st.profile)
        .execute(&db.pool)
        .await
        .expect("unbind the tag service");
    }
    egret_perm_compile::recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        st.profile,
    )
    .await
    .expect("compile");
    set_mechanism(&db, st.profile, mechanism).await;
    let (app, token) = app(&db, None).await;
    let (status, body, _) = explain(
        &app,
        &token,
        &st.ann,
        "read",
        target(st.cat, Some("t"), Some("email")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    only_profile(&body).clone()
}

#[tokio::test]
async fn a_tagless_trino_profile_is_explained_with_the_no_tag_service_warning() {
    for mechanism in ["ranger", "opa"] {
        let p = explain_with_tag_service(mechanism, false).await;
        assert_eq!(p["mechanism"], mechanism, "{p}");
        assert_eq!(
            p["no_tag_service_warning"], NO_TAG_WARNING,
            "{mechanism}: {p}"
        );
    }
}

#[tokio::test]
async fn a_trino_profile_with_a_tag_service_is_explained_without_the_warning() {
    for mechanism in ["ranger", "opa"] {
        let p = explain_with_tag_service(mechanism, true).await;
        assert_eq!(p["mechanism"], mechanism, "{p}");
        assert!(
            p.get("no_tag_service_warning").is_none(),
            "{mechanism}: {p}"
        );
    }
}
