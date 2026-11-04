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
use egret_perm_compile::opa::cache::SnapshotCache;
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
    MaskAndFilter,
    UnusableGrant,
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
        })
        .expect("empty snapshot")
    };
    let at_key = Arc::new(SnapshotCache::default());
    at_key.insert_for_test(st.profile, empty(current));
    let stale = Arc::new(SnapshotCache::default());
    stale.insert_for_test(st.profile, empty((current.0 - 1, current.1)));
    let (app_at_key, token) = app(&db, Some(at_key)).await;
    let (app_stale, _) = app(&db, Some(stale)).await;
    let id = target(st.cat, Some("t"), Some("id"));

    // Act
    let (_, from_cache, _) = explain(&app_at_key, &token, &st.bob, "read", id.clone()).await;
    let (_, rebuilt, _) = explain(&app_stale, &token, &st.bob, "read", id).await;

    // Assert
    let a = access(only_profile(&from_cache), "select");
    assert_eq!(
        a["decision"]["kind"], "denied",
        "the cached snapshot at the current key answered: {a}"
    );
    let a = access(only_profile(&rebuilt), "select");
    assert_eq!(
        a["decision"]["kind"], "allowed",
        "a stale cached snapshot is not used; explain built a fresh one: {a}"
    );
}
